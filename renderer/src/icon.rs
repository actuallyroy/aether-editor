// A shared texture atlas + instanced renderer for extension icons. Decoded icon
// pixels are packed into one atlas texture (simple shelf packing); each icon gets
// a normalized UV sub-rect, so all icons draw in a single instanced pass with one
// bind group. Icons that don't fit or fail to decode simply get no slot, and the
// caller falls back to a colored placeholder tile.

use std::collections::HashMap;

use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BlendState, Buffer, BufferDescriptor, BufferUsages,
    ColorTargetState, ColorWrites, Device, Extent3d, FilterMode, FragmentState, MultisampleState,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass,
    RenderPipeline, RenderPipelineDescriptor, SamplerBindingType, SamplerDescriptor,
    ImageCopyTexture, ImageDataLayout, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages, TextureViewDescriptor, TextureViewDimension, VertexAttribute,
    VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IconInstance {
    pub rect: [f32; 4], // x, y, w, h (pixels)
    pub uv: [f32; 4],   // u0, v0, du, dv (normalized)
}

const ATLAS_SIZE: u32 = 1024;
const PAD: u32 = 2;

pub struct IconAtlas {
    pipeline: RenderPipeline,
    texture: Texture,
    bind_group: BindGroup,
    uniform_buf: Buffer,
    instance_buf: Buffer,
    capacity_bytes: u64,
    count: u32,
    // Shelf-packing cursor.
    cursor_x: u32,
    cursor_y: u32,
    shelf_h: u32,
    slots: HashMap<String, [f32; 4]>,
}

impl IconAtlas {
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("icon-shader"),
            source: ShaderSource::Wgsl(include_str!("icon.wgsl").into()),
        });

        let texture = device.create_texture(&TextureDescriptor {
            label: Some("icon-atlas"),
            size: Extent3d { width: ATLAS_SIZE, height: ATLAS_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("icon-sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        let bind_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("icon-bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_buf = device.create_buffer(&BufferDescriptor {
            label: Some("icon-uniform"),
            size: 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("icon-bg"),
            layout: &bind_layout,
            entries: &[
                BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() },
                BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("icon-pl"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("icon-pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[VertexBufferLayout {
                    array_stride: std::mem::size_of::<IconInstance>() as u64,
                    step_mode: VertexStepMode::Instance,
                    attributes: &[
                        VertexAttribute { offset: 0, shader_location: 0, format: VertexFormat::Float32x4 },
                        VertexAttribute { offset: 16, shader_location: 1, format: VertexFormat::Float32x4 },
                    ],
                }],
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: "fs_main",
                compilation_options: PipelineCompilationOptions::default(),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let capacity_bytes = 128 * std::mem::size_of::<IconInstance>() as u64;
        let instance_buf = device.create_buffer(&BufferDescriptor {
            label: Some("icon-instances"),
            size: capacity_bytes,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            texture,
            bind_group,
            uniform_buf,
            instance_buf,
            capacity_bytes,
            count: 0,
            cursor_x: 0,
            cursor_y: 0,
            shelf_h: 0,
            slots: HashMap::new(),
        }
    }

    /// Look up an already-packed icon's UV rect.
    pub fn get(&self, key: &str) -> Option<[f32; 4]> {
        self.slots.get(key).copied()
    }

    /// Decode a raster icon file and pack it into the atlas, returning its UV rect.
    pub fn load(&mut self, queue: &Queue, key: &str, path: &std::path::Path) -> Option<[f32; 4]> {
        if let Some(uv) = self.slots.get(key) {
            return Some(*uv);
        }
        let img = image::open(path).ok()?;
        self.add_image(queue, key, img)
    }

    /// Decode raster icon bytes (e.g. a downloaded PNG) and pack into the atlas.
    pub fn load_bytes(&mut self, queue: &Queue, key: &str, bytes: &[u8]) -> Option<[f32; 4]> {
        if let Some(uv) = self.slots.get(key) {
            return Some(*uv);
        }
        let img = image::load_from_memory(bytes).ok()?;
        self.add_image(queue, key, img)
    }

    /// Pack a decoded image into the atlas, returning its UV rect. Returns None on
    /// a zero-size image or a full atlas.
    fn add_image(&mut self, queue: &Queue, key: &str, img: image::DynamicImage) -> Option<[f32; 4]> {
        // Downscale oversized icons so the atlas holds them all (icons render ~42px).
        let img = if img.width() > 96 || img.height() > 96 {
            img.resize(96, 96, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        // Shelf packing.
        if self.cursor_x + w + PAD > ATLAS_SIZE {
            self.cursor_x = 0;
            self.cursor_y += self.shelf_h + PAD;
            self.shelf_h = 0;
        }
        if self.cursor_y + h > ATLAS_SIZE {
            return None; // atlas full
        }
        let (x, y) = (self.cursor_x, self.cursor_y);
        queue.write_texture(
            ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.cursor_x += w + PAD;
        self.shelf_h = self.shelf_h.max(h);
        let s = ATLAS_SIZE as f32;
        let uv = [x as f32 / s, y as f32 / s, w as f32 / s, h as f32 / s];
        self.slots.insert(key.to_string(), uv);
        Some(uv)
    }

    pub fn prepare(&mut self, device: &Device, queue: &Queue, instances: &[IconInstance], res: (u32, u32)) {
        let bytes: &[u8] = bytemuck::cast_slice(instances);
        let needed = bytes.len() as u64;
        if needed > self.capacity_bytes {
            self.capacity_bytes = needed.next_power_of_two().max(128 * 32);
            self.instance_buf = device.create_buffer(&BufferDescriptor {
                label: Some("icon-instances"),
                size: self.capacity_bytes,
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.instance_buf, 0, bytes);
        }
        let uniform = [res.0 as f32, res.1 as f32, 0.0, 0.0];
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&uniform));
        self.count = instances.len() as u32;
    }

    pub fn render<'a>(&'a self, pass: &mut RenderPass<'a>) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buf.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}
