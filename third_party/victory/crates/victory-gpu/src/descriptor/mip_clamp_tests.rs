use super::*;

#[test]
fn srv_adjustment_shifts_mips_and_clamps_lod_for_2d_and_2d_array() {
    let adjusted = |most, mips, lod| {
        let mut desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: most,
                    MipLevels: mips,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: lod,
                },
            },
        };
        adjust_srv_for_base_mip(&mut desc, 2, 10);
        // SAFETY: the 2D member was initialized above.
        let view = unsafe { desc.Anonymous.Texture2D };
        (view.MostDetailedMip, view.MipLevels, view.ResourceMinLODClamp)
    };
    assert_eq!(adjusted(0, u32::MAX, 0.0), (0, u32::MAX, 0.0));
    assert_eq!(adjusted(0, 12, 0.0), (0, 10, 0.0));
    assert_eq!(adjusted(0, 1, 0.5), (0, 1, 0.0));
    assert_eq!(adjusted(1, 4, 2.5), (0, 3, 0.5));
    assert_eq!(adjusted(4, 3, 5.25), (2, 3, 3.25));
    assert_eq!(adjusted(40, 30, f32::NAN), (9, 1, 0.0));

    let mut array = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2DARRAY,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2DArray: D3D12_TEX2D_ARRAY_SRV {
                MostDetailedMip: 3,
                MipLevels: 2,
                FirstArraySlice: 0,
                ArraySize: 1,
                PlaneSlice: 0,
                ResourceMinLODClamp: 99.0,
            },
        },
    };
    adjust_srv_for_base_mip(&mut array, 2, 8);
    // SAFETY: the 2D array member was initialized above.
    let view = unsafe { array.Anonymous.Texture2DArray };
    assert_eq!((view.MostDetailedMip, view.MipLevels, view.ResourceMinLODClamp), (1, 2, 2.0));
    assert_eq!((view.FirstArraySlice, view.ArraySize, view.PlaneSlice), (0, 1, 0));
}
