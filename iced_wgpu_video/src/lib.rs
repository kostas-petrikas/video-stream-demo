//! WGPU based video player widget for Iced

pub use ffmpeg_next::frame::Video as VideoFrame;
use iced::widget::shader::{Pipeline, Primitive, Viewport};
use iced::{Element, Length, Size, wgpu};
use iced_wgpu::primitive::Renderer;
use std::sync::Arc;
use std::time::SystemTime;
use wgpu::util::DeviceExt;

// persist for the same widget between view() calls
type VideoWidgetCache = Arc<spin::Mutex<Option<Cache>>>;

#[derive(Clone, Debug)]
pub struct VideoBuffer {
    pub y: wgpu::Texture,
    pub u: wgpu::Texture,
    pub v: wgpu::Texture,
    pub bind_group: wgpu::BindGroup,
}

#[derive(Clone, Debug)]
pub struct CameraBuffer {
    pub buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

#[derive(Clone, Debug)]
pub struct Cache {
    pub animated_at: Option<SystemTime>,
    pub video: VideoBuffer,
    pub camera: CameraBuffer,
}

impl Cache {
    pub fn new(device: &wgpu::Device, frame: Size<u32>) -> Self {
        Self {
            animated_at: None,
            video: VideoBuffer::new(device, frame),
            camera: Camera::buffer(device),
        }
    }
}

impl VideoBuffer {
    pub fn new(device: &wgpu::Device, frame: Size<u32>) -> Self {
        let y = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Video Y"),
            size: wgpu::Extent3d {
                width: frame.width,
                height: frame.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let u = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Video U"),
            size: wgpu::Extent3d {
                width: frame.width / 2,
                height: frame.height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let v = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Video V"),
            size: wgpu::Extent3d {
                width: frame.width / 2,
                height: frame.height / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let layout = &Self::layout(device);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Video textures"),
            layout: layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &y.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &u.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &v.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            y,
            u,
            v,
            bind_group,
        }
    }

    pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Video textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        })
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view_proj: [[f32; 4]; 4],
}

impl Camera {
    pub fn new(animated_at: Option<SystemTime>) -> Self {
        let view_proj = if let Some(animated_at) = animated_at {
            const RAD: f32 = 4.0; // camera distance
            let delta = SystemTime::now()
                .duration_since(animated_at)
                // we trust the clock source
                .unwrap();

            let angle = delta.as_secs_f32() * 0.5;
            let cam_x = angle.sin() * RAD;
            let cam_z = angle.cos() * RAD;
            let cam_y = angle.cos() * RAD - 2.0;

            let view = glam::Mat4::look_at_rh(
                glam::Vec3::new(cam_x, cam_y, cam_z),
                glam::Vec3::new(0.0, 0.0, 0.5),
                glam::Vec3::Y,
            );

            let proj = glam::Mat4::perspective_rh(45.0f32.to_radians(), 1.0, 0.1, 100.0);

            // Convert from OGL to WGPU coordinate systems
            let opengl_to_wgpu = glam::Mat4::from_cols_array(&[
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 1.0,
            ]);

            (opengl_to_wgpu * proj * view).to_cols_array_2d()
        } else {
            glam::Mat4::IDENTITY.to_cols_array_2d()
        };

        Self { view_proj }
    }

    pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Camera uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        })
    }

    pub fn buffer(device: &wgpu::Device) -> CameraBuffer {
        let this = Self::new(None);
        let layout = Self::layout(device);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Uniform Buffer"),
            contents: bytemuck::cast_slice(&[this.view_proj]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: Some("Camera Bind Group"),
        });

        CameraBuffer { buffer, bind_group }
    }
}

pub struct VideoPipeline(wgpu::RenderPipeline);

impl Pipeline for VideoPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Video shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let buffer_layout = VideoBuffer::layout(device);
        let camera_layout = Camera::layout(device);

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Video pipeline"),
            bind_group_layouts: &[&camera_layout, &buffer_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Video pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Front),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multiview: None,
            multisample: Default::default(),
            cache: None,
        });

        Self(pipeline)
    }
}

pub struct VideoPrimitive {
    video_frame: Option<VideoFrame>,
    cache: VideoWidgetCache,
}

impl core::fmt::Debug for VideoPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPrimitive").finish()
    }
}

impl VideoPrimitive {
    pub fn new(video_frame: Option<VideoFrame>, cache: VideoWidgetCache) -> Self {
        Self { video_frame, cache }
    }
}

impl Primitive for VideoPrimitive {
    type Pipeline = VideoPipeline;

    fn prepare(
        &self,
        _pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &iced::Rectangle,
        _viewport: &Viewport,
    ) {
        if let Some(video_frame) = &self.video_frame {
            let buffs = {
                let mut cache = self.cache.lock();

                match &*cache {
                    Some(vbuff) => vbuff.clone(),
                    None => {
                        let new_cache = Cache::new(
                            device,
                            Size {
                                width: video_frame.width(),
                                height: video_frame.height(),
                            },
                        );
                        cache.replace(new_cache.clone());
                        new_cache
                    }
                }
            };

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &buffs.video.y,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &video_frame.data(0),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(video_frame.width()),
                    rows_per_image: Some(video_frame.height()),
                },
                wgpu::Extent3d {
                    width: video_frame.width(),
                    height: video_frame.height(),
                    depth_or_array_layers: 1,
                },
            );

            let uv_width = video_frame.width() / 2;
            let uv_height = video_frame.height() / 2;
            let uv_size = wgpu::Extent3d {
                width: uv_width,
                height: uv_height,
                depth_or_array_layers: 1,
            };

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &buffs.video.u,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                video_frame.data(1),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(uv_width),
                    rows_per_image: Some(uv_height),
                },
                uv_size,
            );

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &buffs.video.v,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                video_frame.data(2),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(uv_width),
                    rows_per_image: Some(uv_height),
                },
                uv_size,
            );

            let camera = Camera::new(buffs.animated_at);

            queue.write_buffer(
                &buffs.camera.buffer,
                0,
                bytemuck::cast_slice(&[camera.view_proj]),
            );
        }
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        bounds: &iced::Rectangle<u32>,
    ) {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        if let Some(guard) = self.cache.try_lock()
            && let Some(cache) = &*guard
        {
            render_pass.set_pipeline(&pipeline.0);
            render_pass.set_bind_group(0, &cache.camera.bind_group, &[]);
            render_pass.set_bind_group(1, &cache.video.bind_group, &[]);
            render_pass.set_viewport(
                bounds.x as _,
                bounds.y as _,
                bounds.width as _,
                bounds.height as _,
                0.0,
                1.0,
            );
            render_pass.draw(0..36, 0..1);
        }
    }
}

pub struct VideoWidget<'a> {
    width: Length,
    height: Length,
    video_frame: &'a Option<VideoFrame>,
}

impl<'a> VideoWidget<'a> {
    pub fn new(width: Length, height: Length, video_frame: &'a Option<VideoFrame>) -> Self {
        Self {
            width,
            height,
            video_frame,
        }
    }
}

impl<'a, Message, Theme> iced::advanced::Widget<Message, Theme, iced::Renderer>
    for VideoWidget<'a>
{
    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(VideoWidgetCache::default())
    }

    fn size(&self) -> Size<iced::Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(
        &mut self,
        _tree: &mut iced::advanced::widget::Tree,
        _renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        iced::advanced::layout::atomic(limits, self.width, self.height)
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout<'_>,
        _cursor: iced::advanced::mouse::Cursor,
        _viewport: &iced::Rectangle,
    ) {
        let cache = tree.state.downcast_ref::<VideoWidgetCache>();
        renderer.draw_primitive(
            layout.bounds(),
            VideoPrimitive::new(self.video_frame.clone(), cache.clone()),
        );
    }
    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        _layout: iced::advanced::Layout<'_>,
        _cursor: iced::advanced::mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn iced::advanced::Clipboard,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &iced::Rectangle,
    ) {
        if let iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_)) = event {
            let cache = tree.state.downcast_ref::<VideoWidgetCache>();
            let mut guard = cache.lock();

            if let Some(c) = guard.as_mut() {
                c.animated_at = Some(SystemTime::now());
            }
        }

        shell.request_redraw();
    }
}

impl<'a, Message, Theme> From<VideoWidget<'a>> for Element<'a, Message, Theme, iced::Renderer>
where
    Message: 'a,
{
    fn from(custom: VideoWidget<'a>) -> Element<'a, Message, Theme, iced::Renderer> {
        Element::new(custom)
    }
}
