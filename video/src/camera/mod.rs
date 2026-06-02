//! Async camera AV1 streaming implementation

use ffmpeg::format::pixel::Pixel::{YUV420P, YUYV422};
use ffmpeg::software::scaling;
use ffmpeg_next as ffmpeg;
use futures::Stream;
use nokhwa;
use nokhwa::utils::Resolution;
use rtc::{media::Sample, shared::time::SystemInstant};
use tokio::sync::mpsc::{Receiver, Sender, channel};

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("Nohkwa error: {0:?}")]
    Nokhwa(nokhwa::NokhwaError),
    #[error("No cameras found")]
    NoCamera,
    #[error("Ffmpeg error: {0:?}")]
    Ffmpeg(ffmpeg::Error),
    #[error("Could not find video encoder")]
    NoEncoder,
}

pub type CameraResult = Result<Sample, Error>;

/// Camera pipeline
/// Poll plain camera YUVY frame -> scale to 720p -> convert to YUV420p -> write to AV1 encoder -> try to poll AV1 packet
struct CameraEncoder {
    video: ffmpeg::encoder::Video,
    converter: ffmpeg::software::scaling::Context,
    yuv420p_input_frame: ffmpeg::frame::Video,
    camera_res: (u32, u32),
}

impl CameraEncoder {
    pub fn new(width: u32, height: u32) -> Result<Self, Error> {
        let codec = ffmpeg::codec::encoder::find_by_name("libsvtav1").ok_or(Error::NoEncoder)?;

        let mut encoder = ffmpeg::codec::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(Error::Ffmpeg)?;

        // Params tuned manually, can be improved
        // TODO: add support for dynamic tuning based on RTCP feedback, curently hardcoded for 720p
        // TODO: support layered encoding
        let mut ops = ffmpeg::Dictionary::new();
        ops.set("preset", "8");
        ops.set("tbr", "1000");
        ops.set("mbr", "2000");

        let svt_params = [
            "rtc=1",
            "enable-overlays=0",
            "scd=0",
            "lookahead=0",
            "pred-struct=1",
            "fast-decode=1",
        ]
        .join(":");
        ops.set("svtav1-params", &svt_params);

        encoder.set_width(1280);
        encoder.set_height(720);
        encoder.set_format(YUV420P);
        encoder.set_frame_rate(Some((30, 1)));
        encoder.set_time_base((1, 30));
        encoder.set_max_b_frames(0);
        // encoder.set_gop(30);
        // encoder.set_bit_rate(1000_000);
        encoder.set_flags(ffmpeg::codec::Flags::LOW_DELAY);

        let video = encoder.open_with(ops).map_err(Error::Ffmpeg)?;

        let converter = scaling::Context::get(
            YUYV422,
            width,
            height,
            YUV420P,
            1280,
            720,
            scaling::flag::Flags::FAST_BILINEAR,
        )
        .map_err(Error::Ffmpeg)?;

        let yuv420p_input_frame = ffmpeg::frame::Video::new(YUV420P, 1280, 720);

        Ok(Self {
            video,
            converter,
            yuv420p_input_frame,
            camera_res: (width, height),
        })
    }

    pub fn encode(&mut self, frame: &[u8]) -> Result<(), Error> {
        let mut yuyv422_input_frame =
            ffmpeg::frame::Video::new(YUYV422, self.camera_res.0, self.camera_res.1);
        yuyv422_input_frame.data_mut(0).copy_from_slice(frame);

        self.converter
            .run(&yuyv422_input_frame, &mut self.yuv420p_input_frame)
            .map_err(Error::Ffmpeg)?;

        let pts = self.yuv420p_input_frame.pts().map(|pts| pts + 1);
        self.yuv420p_input_frame.set_pts(pts);

        self.video
            .send_frame(&self.yuv420p_input_frame)
            .map_err(Error::Ffmpeg)?;

        Ok(())
    }

    pub fn read(&mut self) -> Option<ffmpeg::Packet> {
        let mut packet = ffmpeg::Packet::empty();

        if self.video.receive_packet(&mut packet).is_ok() {
            Some(packet)
        } else {
            None
        }
    }

    pub fn read_sample(&mut self) -> Option<Sample> {
        self.read().map(|packet| {
            let time_base = self.video.time_base();
            let data = bytes::Bytes::copy_from_slice(packet.data().unwrap_or_default());
            let duration_secs = packet.duration() as f64 * f64::from(time_base.numerator())
                / f64::from(time_base.denominator());
            let duration = std::time::Duration::from_secs_f64(duration_secs);

            Sample {
                data,
                timestamp: SystemInstant::now(),
                duration,
                ..Default::default()
            }
        })
    }
}

/// Combine camera device with camera pipeline
struct CameraFeed {
    camera: nokhwa::Camera,
    encoder: CameraEncoder,
}

impl CameraFeed {
    pub fn run(frame_tx: Sender<CameraResult>) {
        let mut this = match Self::new() {
            Err(err) => {
                #[allow(unused)]
                frame_tx.blocking_send(Err(err));
                return;
            }
            Ok(this) => this,
        };

        if let Err(err) = this.feed_loop(&frame_tx) {
            #[allow(unused)]
            frame_tx.blocking_send(Err(err));
            return;
        }
    }

    fn feed_loop(&mut self, frame_tx: &Sender<CameraResult>) -> Result<(), Error> {
        loop {
            let camera_frame = self.camera.frame().map_err(Error::Nokhwa)?;

            self.encoder.encode(camera_frame.buffer())?;
            while let Some(sample) = self.encoder.read_sample() {
                if frame_tx.blocking_send(Ok(sample)).is_err() {
                    return Ok(());
                }
            }
        }
    }

    #[inline]
    fn new() -> Result<Self, Error> {
        let camera = Self::init_camera()?;
        let Resolution { width_x, height_y } = camera.resolution();
        println!("CAMERA: {width_x} {height_y}");
        let encoder = CameraEncoder::new(width_x, height_y)?;

        Ok(Self { camera, encoder })
    }

    fn init_camera() -> Result<nokhwa::Camera, Error> {
        let cameras = nokhwa::query(nokhwa::utils::ApiBackend::Auto).map_err(Error::Nokhwa)?;
        let camera = cameras.first().ok_or(Error::NoCamera)?;
        let format = nokhwa::utils::RequestedFormat::new::<nokhwa::pixel_format::YuyvFormat>(
            nokhwa::utils::RequestedFormatType::AbsoluteHighestFrameRate,
        );

        let mut camera =
            nokhwa::Camera::new(camera.index().clone(), format).map_err(Error::Nokhwa)?;

        camera.open_stream().map_err(Error::Nokhwa)?;

        Ok(camera)
    }
}

pin_project_lite::pin_project! {
    /// Async stream interface for camera pipeline
    pub struct CameraStream {
        #[pin]
        rx: Receiver<CameraResult>
    }
}

impl CameraStream {
    pub fn new() -> Self {
        // TODO: while it is lockless it might potentially cause camera to crash if CPU is under heavy load,
        // because the device might not tolerate long waiting times, try to load-test
        let (frame_tx, frame_rx) = channel(1);

        std::thread::spawn(move || CameraFeed::run(frame_tx));

        Self { rx: frame_rx }
    }
}

impl Stream for CameraStream {
    type Item = CameraResult;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut this = self.project();

        this.rx.poll_recv(cx)
    }
}
