use std::{mem::ManuallyDrop, ptr, slice};
use windows::{
    Win32::{
        Foundation::{CloseHandle, RECT},
        Graphics::{Direct3D::*, Direct3D12::*, Dxgi::Common::*},
        System::Threading::{CreateEventW, INFINITE, WaitForSingleObject},
    },
    core::{Interface, Result},
};

pub struct Draw {
    pub vertices: u32,
    pub width: u32,
    pub height: u32,
    pub pixel: bool,
}

pub fn root(device: &ID3D12Device) -> Result<ID3D12RootSignature> {
    let param = D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        ..Default::default()
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: 1,
        pParameters: &param,
        ..Default::default()
    };
    let mut blob = None;
    unsafe {
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)?;
        let blob = blob.expect("root signature blob");
        device.CreateRootSignature(
            0,
            slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()),
        )
    }
}
fn bytecode(bytes: &[u8]) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE {
        pShaderBytecode: if bytes.is_empty() {
            ptr::null()
        } else {
            bytes.as_ptr().cast()
        },
        BytecodeLength: bytes.len(),
    }
}
pub fn compute(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    shader: &[u8],
) -> Result<ID3D12PipelineState> {
    let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        CS: bytecode(shader),
        ..Default::default()
    };
    let result = unsafe { device.CreateComputePipelineState(&desc) };
    unsafe {
        ManuallyDrop::into_inner(ptr::read(&desc.pRootSignature));
    }
    result
}
pub fn graphics(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    vs: &[u8],
    ps: &[u8],
) -> Result<ID3D12PipelineState> {
    let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        VS: bytecode(vs),
        PS: bytecode(ps),
        SampleMask: u32::MAX,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: u32::from(!ps.is_empty()),
        ..Default::default()
    };
    desc.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
    desc.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    desc.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    desc.RasterizerState.DepthClipEnable = true.into();
    if !ps.is_empty() {
        desc.RTVFormats[0] = DXGI_FORMAT_R32G32B32A32_FLOAT;
    }
    desc.SampleDesc.Count = 1;
    let result = unsafe { device.CreateGraphicsPipelineState(&desc) };
    unsafe {
        ManuallyDrop::into_inner(ptr::read(&desc.pRootSignature));
    }
    result
}
fn resource(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: Option<&D3D12_CLEAR_VALUE>,
) -> Result<ID3D12Resource> {
    let props = D3D12_HEAP_PROPERTIES {
        Type: heap,
        ..Default::default()
    };
    let mut result = None;
    unsafe {
        device.CreateCommittedResource(
            &props,
            D3D12_HEAP_FLAG_NONE,
            desc,
            state,
            clear.map(|v| v as *const _),
            &mut result,
        )?;
    }
    Ok(result.expect("created resource"))
}
fn buffer(
    device: &ID3D12Device,
    heap: D3D12_HEAP_TYPE,
    size: u64,
    flags: D3D12_RESOURCE_FLAGS,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource> {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    };
    resource(device, heap, &desc, state, None)
}
fn transition(
    list: &ID3D12GraphicsCommandList,
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(resource.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
        ..Default::default()
    };
    unsafe {
        list.ResourceBarrier(slice::from_ref(&barrier));
        ManuallyDrop::into_inner(ptr::read(&barrier.Anonymous.Transition.pResource));
    }
}
pub fn execute(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    pipeline: &ID3D12PipelineState,
    memory: &mut [u8],
    groups: u32,
    draw: Option<&Draw>,
) -> Result<()> {
    let size = memory.len() as u64;
    let gpu = buffer(
        device,
        D3D12_HEAP_TYPE_DEFAULT,
        size,
        D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        D3D12_RESOURCE_STATE_COPY_DEST,
    )?;
    let upload = buffer(
        device,
        D3D12_HEAP_TYPE_UPLOAD,
        size,
        D3D12_RESOURCE_FLAG_NONE,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    let readback = buffer(
        device,
        D3D12_HEAP_TYPE_READBACK,
        size,
        D3D12_RESOURCE_FLAG_NONE,
        D3D12_RESOURCE_STATE_COPY_DEST,
    )?;
    unsafe {
        let mut mapped = ptr::null_mut();
        upload.Map(0, None, Some(&mut mapped))?;
        ptr::copy_nonoverlapping(memory.as_ptr(), mapped.cast(), memory.len());
        upload.Unmap(0, None);
        let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            ..Default::default()
        })?;
        let allocator: ID3D12CommandAllocator =
            device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
        let list: ID3D12GraphicsCommandList =
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, pipeline)?;
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
        list.CopyBufferRegion(&gpu, 0, &upload, 0, size);
        transition(
            &list,
            &gpu,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        );
        let mut target = None;
        let mut heap = None;
        if let Some(draw) = draw {
            list.SetGraphicsRootSignature(root);
            list.SetGraphicsRootUnorderedAccessView(0, gpu.GetGPUVirtualAddress());
            list.RSSetViewports(&[D3D12_VIEWPORT {
                Width: draw.width as f32,
                Height: draw.height as f32,
                MaxDepth: 1.0,
                ..Default::default()
            }]);
            list.RSSetScissorRects(&[RECT {
                left: 0,
                top: 0,
                right: draw.width as i32,
                bottom: draw.height as i32,
            }]);
            if draw.pixel {
                let desc = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: draw.width as u64,
                    Height: draw.height,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                    ..Default::default()
                };
                let clear = D3D12_CLEAR_VALUE {
                    Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                    ..Default::default()
                };
                let tex = resource(
                    device,
                    D3D12_HEAP_TYPE_DEFAULT,
                    &desc,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    Some(&clear),
                )?;
                let views: ID3D12DescriptorHeap =
                    device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                        NumDescriptors: 1,
                        ..Default::default()
                    })?;
                let view = views.GetCPUDescriptorHandleForHeapStart();
                device.CreateRenderTargetView(&tex, None, view);
                list.OMSetRenderTargets(1, Some(&view), false, None);
                list.ClearRenderTargetView(view, &[0.0; 4], None);
                target = Some(tex);
                heap = Some(views);
            }
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.DrawInstanced(draw.vertices, 1, 0, 0);
        } else {
            list.SetComputeRootSignature(root);
            list.SetComputeRootUnorderedAccessView(0, gpu.GetGPUVirtualAddress());
            list.Dispatch(groups, 1, 1);
        }
        transition(
            &list,
            &gpu,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        );
        list.CopyBufferRegion(&readback, 0, &gpu, 0, size);
        list.Close()?;
        queue.ExecuteCommandLists(&[Some(list.cast()?)]);
        let event = CreateEventW(None, false, false, None)?;
        let wait_result: Result<()> = (|| {
            queue.Signal(&fence, 1)?;
            fence.SetEventOnCompletion(1, event)?;
            WaitForSingleObject(event, INFINITE);
            Ok(())
        })();
        let _ = CloseHandle(event);
        wait_result?;
        device.GetDeviceRemovedReason()?;
        readback.Map(0, None, Some(&mut mapped))?;
        ptr::copy_nonoverlapping(mapped.cast(), memory.as_mut_ptr(), memory.len());
        readback.Unmap(0, None);
        drop((target, heap));
    }
    Ok(())
}
