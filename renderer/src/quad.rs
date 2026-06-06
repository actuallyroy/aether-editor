// Solid-color rectangle renderer. Instanced.
// Two ranges in one instance buffer: bg quads (drawn before text) and
// fg quads (drawn after text), so selection sits under glyphs and cursor on top.

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BlendState, Buffer, BufferBindingType,
    BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, Device, FragmentState,
    MultisampleState, PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState,
    Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor,
    ShaderSource, ShaderStages, TextureFormat, VertexAttribute, VertexBufferLayout,
    VertexFormat, VertexState, VertexStepMode,
};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Quad {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub params: [f32; 4], // params[0] = corner radius (px); 0 = sharp rectangle
}

impl Quad {
    pub fn new(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Self {
        Self {
            rect: [x, y, w, h],
            color,
            params: [0.0; 4],
        }
    }

    /// A rectangle with rounded corners of `radius` px (clamped to half the
    /// smaller side in the shader). radius 0 is identical to `new`.
    pub fn rounded(x: f32, y: f32, w: f32, h: f32, color: [f32; 4], radius: f32) -> Self {
        Self {
            rect: [x, y, w, h],
            color,
            params: [radius, 0.0, 0.0, 0.0],
        }
    }

    /// A circular ring band (outer..inner radius) centered at `(cx, cy)` in pixels,
    /// drawn only within rect `(x,y,w,h)` — so a quadrant-sized rect yields a quarter
    /// arc. Used for curved graph connectors. `inner` must be > 0.
    pub fn arc(x: f32, y: f32, w: f32, h: f32, color: [f32; 4], outer: f32, inner: f32, cx: f32, cy: f32) -> Self {
        // Center is passed in local (rect-relative) px so the shader needs no origin.
        Self {
            rect: [x, y, w, h],
            color,
            params: [outer, inner.max(0.01), cx - x, cy - y],
        }
    }
}

pub struct QuadRenderer {
    pipeline: RenderPipeline,
    instance_buf: Buffer,
    capacity_bytes: u64,
    uniform_buf: Buffer,
    bind_group: BindGroup,
    bg_count: u32,
    fg_count: u32,
}

impl QuadRenderer {
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("quad-shader"),
            source: ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("quad-bgl"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_buf = device.create_buffer(&BufferDescriptor {
            label: Some("quad-uniform"),
            size: 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("quad-bg"),
            layout: &bind_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("quad-pl"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("quad-pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[VertexBufferLayout {
                    array_stride: std::mem::size_of::<Quad>() as u64,
                    step_mode: VertexStepMode::Instance,
                    attributes: &[
                        VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: VertexFormat::Float32x4,
                        },
                        VertexAttribute {
                            offset: 16,
                            shader_location: 1,
                            format: VertexFormat::Float32x4,
                        },
                        VertexAttribute {
                            offset: 32,
                            shader_location: 2,
                            format: VertexFormat::Float32x4,
                        },
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

        let capacity_bytes = 256 * std::mem::size_of::<Quad>() as u64;
        let instance_buf = device.create_buffer(&BufferDescriptor {
            label: Some("quad-instances"),
            size: capacity_bytes,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            instance_buf,
            capacity_bytes,
            uniform_buf,
            bind_group,
            bg_count: 0,
            fg_count: 0,
        }
    }

    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        bg: &[Quad],
        fg: &[Quad],
        res: (u32, u32),
    ) {
        let mut all: Vec<Quad> = Vec::with_capacity(bg.len() + fg.len());
        all.extend_from_slice(bg);
        all.extend_from_slice(fg);

        let bytes = bytemuck::cast_slice(&all);
        let needed = bytes.len() as u64;
        if needed > self.capacity_bytes {
            self.capacity_bytes = needed.next_power_of_two().max(256 * 32);
            self.instance_buf = device.create_buffer(&BufferDescriptor {
                label: Some("quad-instances"),
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
        self.bg_count = bg.len() as u32;
        self.fg_count = fg.len() as u32;
    }

    pub fn render_bg<'a>(&'a self, pass: &mut RenderPass<'a>) {
        if self.bg_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buf.slice(..));
        pass.draw(0..6, 0..self.bg_count);
    }

    pub fn render_fg<'a>(&'a self, pass: &mut RenderPass<'a>) {
        if self.fg_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buf.slice(..));
        pass.draw(0..6, self.bg_count..(self.bg_count + self.fg_count));
    }
}
