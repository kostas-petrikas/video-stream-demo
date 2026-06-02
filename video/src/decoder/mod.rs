pub mod rtp_stream;
pub mod stats;

use ffmpeg_next as ffmpeg;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Ffmpeg error: {0:?}")]
    Ffmpeg(ffmpeg::Error),
    #[error("Could not find decoder codec")]
    NoCodec,
}

pub struct VideoDecoder {
    video: ffmpeg::decoder::Video,
}

impl VideoDecoder {
    pub fn new() -> Result<Self, Error> {
        let codec = ffmpeg::codec::decoder::find_by_name("libdav1d").ok_or(Error::NoCodec)?;
        let mut decoder = ffmpeg::codec::Context::new_with_codec(codec)
            .decoder()
            .video()
            .map_err(Error::Ffmpeg)?;

        decoder.set_frame_rate(Some((30, 1)));
        decoder.set_time_base((1, 90000));
        decoder.set_flags(ffmpeg::codec::Flags::LOW_DELAY);

        Ok(Self { video: decoder })
    }

    pub fn write(&mut self, packet: ffmpeg::Packet) -> Result<(), Error> {
        self.video.send_packet(&packet).map_err(Error::Ffmpeg)
    }

    pub fn read(&mut self) -> Option<ffmpeg::frame::Video> {
        let mut frame = ffmpeg::frame::Video::empty();
        if self.video.receive_frame(&mut frame).is_ok() {
            Some(frame)
        } else {
            None
        }
    }
}
