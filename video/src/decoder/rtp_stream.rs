use super::stats::SumStats;
use core::future::Future;
use core::pin::Pin;
use core::task::Poll;
use futures::Stream;
use rtc::media::io::sample_builder::SampleBuilder;
use rtc::rtp::codec::av1::Av1Depacketizer;
use std::sync::Arc;
use tokio::sync::Mutex;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};

/// RTP pipeline for reading AV1 video
/// poll RTP packet -> write to defragmenter -> try to construct AV1 frame -> write to AV1 decoder -> try to decode full plain frame
pub struct RTPVideoTrack {
    /// RTP track - the source of RTP packets
    track: Arc<dyn TrackRemote + Send + Sync + 'static>,
    /// AV1 frame defragmenter, construct full frames from collection of RTP packets
    collector: SampleBuilder<Av1Depacketizer>,
    /// AV1 to plain frames decoder
    decoder: super::VideoDecoder,
    /// Bandwidth statistics
    pub stats: Arc<SumStats>,
}

impl RTPVideoTrack {
    pub fn new(track: Arc<dyn TrackRemote + Send + Sync + 'static>) -> Self {
        Self {
            track,
            collector: SampleBuilder::new(300, Av1Depacketizer::new(), 90000),
            decoder: super::VideoDecoder::new().unwrap(),
            stats: Arc::new(SumStats::default()),
        }
    }

    fn pull_sample_packet(&mut self) -> Option<ffmpeg_next::Packet> {
        self.collector.pop_with_timestamp().map(|(sample, ts)| {
            let mut ffmpeg_packet = ffmpeg_next::Packet::copy(&sample.data);

            let rtp_ts = ts as i64;
            ffmpeg_packet.set_pts(Some(rtp_ts));
            ffmpeg_packet.set_dts(Some(rtp_ts));

            ffmpeg_packet
        })
    }

    fn pull_frame(&mut self) -> Option<Result<ffmpeg_next::frame::Video, super::Error>> {
        if let Some(frame) = self.decoder.read() {
            return Some(Ok(frame));
        }

        while let Some(sample) = self.pull_sample_packet() {
            if let Err(err) = self.decoder.write(sample) {
                return Some(Err(err));
            }

            if let Some(frame) = self.decoder.read() {
                return Some(Ok(frame));
            }
        }

        None
    }

    pub async fn pull(&mut self) -> Option<Result<ffmpeg_next::frame::Video, super::Error>> {
        if let Some(frame) = self.pull_frame() {
            return Some(frame);
        }

        loop {
            // TODO: handle decoder flushing on stream's end
            let event = self.track.poll().await?;

            match event {
                TrackRemoteEvent::OnRtpPacket(packet) => {
                    self.stats.push(packet.payload.len());
                    self.collector.push(packet);

                    if let Some(frame) = self.pull_frame() {
                        return Some(frame);
                    }
                }
                // TODO: handle errors and mute/unmute
                TrackRemoteEvent::OnError => {}
                TrackRemoteEvent::OnMute => {}
                TrackRemoteEvent::OnUnmute => {}
                _ => {}
            }
        }
    }
}

pin_project_lite::pin_project! {
    /// Async stream wrapper around AV1 RTP pipeline
    /// Easy to use interface: pass RTP track then keep polling for plain frames
    pub struct RTPVideoStream {
        // TODO: its simple and it works, but I want to refactor for a lockless design for more efficiency
        track: Arc<Mutex<RTPVideoTrack>>,
        fut: Option<
            Pin<Box<dyn Future<Output = Option<Result<ffmpeg_next::frame::Video, super::Error>>> + Send + 'static >>,
        >
    }
}

impl RTPVideoStream {
    pub fn new(track: RTPVideoTrack) -> (Self, Arc<SumStats>) {
        let stats = track.stats.clone();
        let track = Arc::new(Mutex::new(track));

        (Self { track, fut: None }, stats)
    }

    async fn fut(
        track: Arc<Mutex<RTPVideoTrack>>,
    ) -> Option<Result<ffmpeg_next::frame::Video, super::Error>> {
        let mut track = track.lock().await;

        track.pull().await
    }
}

impl Stream for RTPVideoStream {
    type Item = Result<ffmpeg_next::frame::Video, super::Error>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.project();

        loop {
            if let Some(fut) = this.fut.as_mut() {
                match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(res) => {
                        *this.fut = None;

                        return Poll::Ready(res);
                    }
                }
            }

            *this.fut = Some(Box::pin(Self::fut(this.track.clone())));
        }
    }
}
