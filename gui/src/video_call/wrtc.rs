//! Very memory safe (Arc/Mutex hell, need to refactor somewhat) Async WebRTC video streaming implementation

use async_channel;
use core::future::Future;
use core::pin::Pin;
use core::task::Poll;
use error_stack::{Report, ResultExt};
use iced::futures::{FutureExt, Stream, StreamExt};
use iced_wgpu_video::VideoFrame;
use portable_atomic::{AtomicU128, Ordering};
use rtc::interceptor::Registry;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::MIME_TYPE_AV1;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
    RtpCodecKind,
};
use signal_proto::RTCIceCandidateInit;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, SetOnce};
use video::camera::CameraStream;
use video::decoder::rtp_stream::{RTPVideoStream, RTPVideoTrack};
use video::decoder::stats::SumStats;
use webrtc::data_channel::DataChannel;
use webrtc::media_stream::Track;
use webrtc::media_stream::{
    MediaStreamTrack, track_local::static_sample::TrackLocalStaticSample, track_remote::TrackRemote,
};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfiguration, RTCConfigurationBuilder, RTCIceServer, RTCPeerConnectionIceEvent,
    RTCPeerConnectionState, RTCSessionDescription,
};
use webrtc::runtime::{Runtime, default_runtime};

// TODO: this is crap (while works), make a faster way to get all local IPs
pub fn get_local_ip() -> std::net::IpAddr {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
        // Google public DNS
        && socket.connect("8.8.8.8:80").is_ok()
        && let Ok(addr) = socket.local_addr()
        && let std::net::IpAddr::V4(ip) = addr.ip()
    {
        ip.into()
    } else {
        std::net::Ipv4Addr::new(127, 0, 0, 1).into()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("WebRTC error")]
pub struct WebRTCError;

fn codec_descr() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: MIME_TYPE_AV1.to_owned(),
        // 90KHz is standard for all video
        clock_rate: 90000,
        sdp_fmtp_line: "".to_owned(),
        ..Default::default()
    }
}

pin_project_lite::pin_project! {
    pub struct TrickleConnectionStream {
        ice_candidates: async_channel::Receiver<RTCIceCandidateInit>,
        connection_done: Arc<SetOnce<RTCPeerConnectionState>>,
        fut: Option<Pin<Box<dyn Future<Output = super::Message> + Send + 'static>>>
    }
}

impl TrickleConnectionStream {
    pub fn new(handler: &PeerConnectionHandler) -> Self {
        Self {
            ice_candidates: handler.ice_candidates.1.clone(),
            connection_done: handler.connection_done.clone(),
            fut: Some(Box::pin(Self::next(
                handler.ice_candidates.1.clone(),
                handler.connection_done.clone(),
            ))),
        }
    }

    async fn next(
        ice_candidates: async_channel::Receiver<RTCIceCandidateInit>,
        connection_done: Arc<SetOnce<RTCPeerConnectionState>>,
    ) -> super::Message {
        loop {
            tokio::select! {
                conn_state = connection_done.wait() => match conn_state {
                    RTCPeerConnectionState::Connected => {
                        return super::Message::Connected
                    },
                    RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                        return super::Message::Disconnected
                    },
                    _ => {}
                },
                candidate = ice_candidates.recv() => if let Ok(candidate) = candidate {
                    return super::Message::SendICECandidate(candidate);
                }
            }
        }
    }
}

impl Stream for TrickleConnectionStream {
    type Item = super::Message;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.project();

        match this.fut {
            None => return Poll::Ready(None),
            Some(fut) => match fut.poll_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(msg) => {
                    match &msg {
                        super::Message::Connected | super::Message::Disconnected => {
                            *this.fut = None;
                        }
                        _ => {
                            *this.fut = Some(Box::pin(Self::next(
                                this.ice_candidates.clone(),
                                this.connection_done.clone(),
                            )));
                        }
                    }

                    return Poll::Ready(Some(msg));
                }
            },
        }
    }
}

#[derive(Clone)]
pub struct PeerConnectionHandler {
    /// Trickle ICE channels
    ice_candidates: (
        async_channel::Sender<RTCIceCandidateInit>,
        async_channel::Receiver<RTCIceCandidateInit>,
    ),
    /// initial connection status received
    connection_done: Arc<SetOnce<RTCPeerConnectionState>>,
    /// disconnection received
    disconnected: SetOnce<()>,
    video_track: SetOnce<Arc<dyn TrackRemote>>,
}

impl PeerConnectionHandler {
    pub fn new() -> Self {
        Self {
            ice_candidates: async_channel::bounded(3),
            connection_done: Arc::new(SetOnce::new()),
            disconnected: SetOnce::new(),
            video_track: SetOnce::new(),
        }
    }

    pub fn trickle(&self) -> TrickleConnectionStream {
        TrickleConnectionStream::new(self)
    }
}

pub enum PeerConnectionEvent {
    Disconnected,
    NewVideoFrame(VideoFrame),
    BandwidthStats(f64),
}

struct PeerConnectionStreamCx {
    handler: Arc<PeerConnectionHandler>,
    stream: Option<RTPVideoStream>,
    bandwidth: Arc<SumStats>,
}

impl PeerConnectionStreamCx {
    async fn get_next(&mut self) -> PeerConnectionEvent {
        loop {
            if let Some(video_stream) = &mut self.stream {
                tokio::select! {
                    frame = video_stream.next() => {
                        match frame {
                            Some(frame) => match frame {
                                Ok(frame) => return PeerConnectionEvent::NewVideoFrame(frame),
                                Err(err) => {
                                    log::error!("{err:?}");
                                    continue;
                                }
                            },
                            None => return PeerConnectionEvent::Disconnected
                        }
                    },
                    _ = self.handler.disconnected.wait() => {
                        return PeerConnectionEvent::Disconnected
                    }
                }
            } else {
                tokio::select! {
                    _ = self.handler.disconnected.wait() => {
                        return PeerConnectionEvent::Disconnected
                    }
                    new_track = self.handler.video_track.wait() => {
                        let (stream, stats) = RTPVideoStream::new(RTPVideoTrack::new(new_track.clone()));
                        self.stream = Some(stream);
                        self.bandwidth = stats;

                        continue;
                    }
                }
            }
        }
    }
}

pin_project_lite::pin_project! {
    pub struct PeerConnectionStream {
        cx: Arc<Mutex<PeerConnectionStreamCx>>,
        last_stats: Arc<AtomicU128>,
        fut: Option<Pin<Box<dyn Future<Output = PeerConnectionEvent> + Send + 'static>>>,
    }
}

impl PeerConnectionStream {
    pub fn new(
        handler: Arc<PeerConnectionHandler>,
        local_track: Arc<TrackLocalStaticSample>,
    ) -> Self {
        tokio::spawn(async move {
            CameraFeedTrack::new(local_track).await.run().await;
            log::debug!("Camera finished")
        });

        let cx = Arc::new(Mutex::new(PeerConnectionStreamCx {
            handler,
            stream: None,
            bandwidth: Arc::new(SumStats::default()),
        }));

        let last_stats = Arc::new(AtomicU128::default());

        Self {
            fut: Some(Box::pin(Self::get_next(last_stats.clone(), cx.clone()))),
            cx,
            last_stats,
        }
    }

    fn handle_stats(last_stats_at: &AtomicU128, stats: &SumStats) -> Option<f64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        let then = last_stats_at.load(Ordering::Relaxed);
        let delta = ((now - then) as f64) / 1000.0;

        if delta > 1.0 {
            let avg = stats.take_sum() as f64;
            last_stats_at.store(now, Ordering::Relaxed);
            return Some(avg / delta);
        }

        None
    }

    async fn get_next(
        last_stats_at: Arc<AtomicU128>,
        cx: Arc<Mutex<PeerConnectionStreamCx>>,
    ) -> PeerConnectionEvent {
        let mut cx = cx.lock().await;

        if let Some(stats) = Self::handle_stats(&last_stats_at, &cx.bandwidth) {
            return PeerConnectionEvent::BandwidthStats(stats);
        }

        cx.get_next().await
    }
}

impl Stream for PeerConnectionStream {
    type Item = PeerConnectionEvent;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.project();

        match this.fut {
            Some(fut) => match fut.as_mut().poll(cx) {
                Poll::Ready(res) => match res {
                    PeerConnectionEvent::Disconnected => {
                        *this.fut = None;
                        Poll::Ready(Some(res))
                    }
                    _ => {
                        *this.fut = Some(Box::pin(Self::get_next(
                            this.last_stats.clone(),
                            this.cx.clone(),
                        )));
                        Poll::Ready(Some(res))
                    }
                },
                Poll::Pending => Poll::Pending,
            },
            None => Poll::Ready(None),
        }
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PeerConnectionHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(candidate) => {
                let _ = self.ice_candidates.0.send(candidate).await;
            }
            Err(error) => log::error!("{error:?}"),
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        match &state {
            RTCPeerConnectionState::Connected
            | RTCPeerConnectionState::Disconnected
            | RTCPeerConnectionState::Closed
            | RTCPeerConnectionState::Failed => {
                let _ = self.connection_done.set(state);

                match &state {
                    RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Closed
                    | RTCPeerConnectionState::Failed => {
                        let _ = self.disconnected.set(());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        log::debug!("WRTC STATE: {state:?}");
    }

    async fn on_data_channel(&self, _data_channel: Arc<dyn DataChannel>) {
        log::debug!("GOT REMOTE CHANNEL");
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        log::debug!("GOT REMOTE TRACK");

        let _ = self.video_track.set(track);
    }
}

#[derive(Clone)]
pub struct VideoPeerConnection {
    pub handler: Arc<PeerConnectionHandler>,
    pub handle: Arc<dyn PeerConnection>,
    /// local video track for writing to
    pub video_track: Arc<TrackLocalStaticSample>,
}

impl VideoPeerConnection {
    /// Wait for connection to get established
    pub async fn await_connection(&self) -> Result<(), Report<WebRTCError>> {
        let status = self.handler.connection_done.wait().await;

        match status {
            RTCPeerConnectionState::Connected => Ok(()),
            _ => Err(Report::new(WebRTCError).attach("Connection closed")),
        }
    }

    /// Wait for disconnection
    pub async fn await_disconnection(&self) {
        self.handler.disconnected.wait().await;
    }
}

impl core::fmt::Debug for VideoPeerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPeerConnection").finish()
    }
}

#[derive(Clone)]
pub struct WebRTCContext {
    media_engine: MediaEngine,
    config: RTCConfiguration,
    runtime: Arc<dyn Runtime>,
}

impl WebRTCContext {
    pub fn new() -> Result<WebRTCContext, Report<WebRTCError>> {
        let mut media_engine = MediaEngine::default();

        let av1_codec = RTCRtpCodecParameters {
            rtp_codec: codec_descr(),
            payload_type: 98, // this is arbitrary non reserved type ID
            ..Default::default()
        };

        media_engine
            .register_codec(av1_codec, RtpCodecKind::Video)
            .change_context(WebRTCError)?;

        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }])
            .build();

        let runtime = default_runtime().expect("No async runtime");

        Ok(WebRTCContext {
            media_engine,
            config,
            runtime,
        })
    }

    pub async fn create_connection(&self) -> Result<VideoPeerConnection, Report<WebRTCError>> {
        let mut media_engine = self.media_engine.clone();
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)
            .change_context(WebRTCError)?;
        let handler = Arc::new(PeerConnectionHandler::new());

        let con = PeerConnectionBuilder::new()
            .with_configuration(self.config.clone())
            .with_handler(handler.clone())
            .with_media_engine(media_engine)
            .with_runtime(self.runtime.clone())
            .with_interceptor_registry(registry)
            .with_udp_addrs(vec![format!("{}:0", get_local_ip())])
            .build()
            .await
            .change_context(WebRTCError)?;

        let ssrc = rand::random::<u32>();
        let video_track = Arc::new(
            TrackLocalStaticSample::new(MediaStreamTrack::new(
                "video".to_owned(),
                "webrtc-rs".to_owned(),
                "video".to_owned(),
                RtpCodecKind::Video,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters {
                        ssrc: Some(ssrc),
                        ..Default::default()
                    },
                    codec: codec_descr(),
                    ..Default::default()
                }],
            ))
            .change_context(WebRTCError)?,
        );

        con.add_track(video_track.clone())
            .await
            .change_context(WebRTCError)?;

        Ok(VideoPeerConnection {
            handler,
            handle: Arc::new(con),
            video_track,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Initiator(pub VideoPeerConnection);

impl Initiator {
    /// create initial offer
    pub async fn offer(&self) -> Result<RTCSessionDescription, Report<WebRTCError>> {
        let offer = self
            .0
            .handle
            .create_offer(None)
            .await
            .change_context(WebRTCError)?;

        self.0
            .handle
            .set_local_description(offer.clone())
            .await
            .change_context(WebRTCError)?;

        // Vanila ICE version:
        // self.0.handler.ice_done.wait().await;

        // self.0
        //     .handle
        //     .local_description()
        //     .await
        //     .ok_or(Report::new(WebRTCError))
        //
        Ok(offer)
    }

    /// process answer to finalize connection
    pub async fn handle_answer(
        &self,
        answer: RTCSessionDescription,
    ) -> Result<(), Report<WebRTCError>> {
        self.0
            .handle
            .set_remote_description(answer)
            .await
            .change_context(WebRTCError)
    }

    pub async fn disconnect(&self) {
        let _ = self.0.handle.close().await;
    }
}

#[derive(Debug, Clone)]
pub struct Follower(pub VideoPeerConnection);

impl Follower {
    /// return answer for the offer
    pub async fn handle_offer(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription, Report<WebRTCError>> {
        self.0
            .handle
            .set_remote_description(offer)
            .await
            .change_context(WebRTCError)?;

        let answer = self
            .0
            .handle
            .create_answer(None)
            .await
            .change_context(WebRTCError)?;

        self.0
            .handle
            .set_local_description(answer.clone())
            .await
            .change_context(WebRTCError)?;

        // Vanila ICE version:
        // self.0.handler.ice_done.wait().await;

        // self.0
        //     .handle
        //     .local_description()
        //     .await
        //     .ok_or(Report::new(WebRTCError))

        Ok(answer)
    }

    pub async fn disconnect(&self) {
        let _ = self.0.handle.close().await;
    }
}

#[derive(Debug, Clone)]
pub enum StatefulPeerConnection {
    /// Connection is initiator and will create initial offer
    Initiator(Initiator),
    /// Connection is follower and will react to initial offer
    Follower(Follower),
}

impl StatefulPeerConnection {
    /// Act as p2p connection initiator, returns initial WebRTC offer
    pub async fn initiate(
        ctx: &WebRTCContext,
    ) -> Result<(Self, RTCSessionDescription), Report<WebRTCError>> {
        let initiator = Initiator(ctx.create_connection().await?);
        let offer = initiator.offer().await?;

        Ok((Self::Initiator(initiator), offer))
    }

    /// Act as p2p follower, exchange initial offer with initial answer
    pub async fn follow(
        ctx: &WebRTCContext,
        initial_offer: RTCSessionDescription,
    ) -> Result<(Self, RTCSessionDescription), Report<WebRTCError>> {
        let follower = Follower(ctx.create_connection().await?);
        let answer = follower.handle_offer(initial_offer).await?;

        Ok((Self::Follower(follower), answer))
    }

    pub fn video_track(&self) -> Arc<TrackLocalStaticSample> {
        match self {
            Self::Follower(conn) => conn.0.video_track.clone(),
            Self::Initiator(conn) => conn.0.video_track.clone(),
        }
    }

    /// Handle ICE candidate trickle in
    pub async fn add_ice_candidate(
        &self,
        candidate: RTCIceCandidateInit,
    ) -> Result<(), Report<WebRTCError>> {
        let handle = match self {
            Self::Follower(connection) => &connection.0.handle,
            Self::Initiator(connection) => &connection.0.handle,
        };

        handle
            .add_ice_candidate(candidate)
            .await
            .change_context(WebRTCError)
    }

    pub fn trickle_connection(&self) -> TrickleConnectionStream {
        let handler = match self {
            Self::Follower(connection) => &connection.0.handler,
            Self::Initiator(connection) => &connection.0.handler,
        };

        handler.trickle()
    }

    pub async fn close(&self) {
        match self {
            Self::Follower(f) => f.disconnect().await,
            Self::Initiator(i) => i.disconnect().await,
        }
    }
}

pub struct CameraFeedTrack {
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    feed: CameraStream,
}

impl CameraFeedTrack {
    pub async fn new(track: Arc<TrackLocalStaticSample>) -> Self {
        let ssrc = *track.ssrcs().await.first().unwrap_or(&0);
        Self {
            track,
            ssrc,
            feed: CameraStream::new(),
        }
    }

    pub async fn run(&mut self) {
        while let Some(Ok(sample)) = self.feed.next().await {
            if self
                .track
                .write_sample(self.ssrc, &sample, &[])
                .await
                .is_err()
            {
                break;
            }
        }

        log::debug!("Camera done");
    }
}
