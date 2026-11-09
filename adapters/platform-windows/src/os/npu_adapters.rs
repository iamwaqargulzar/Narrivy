//! Which display-kernel adapters are NPUs, through DXCore, for
//! `accelerator_usage`. An NPU (an Intel Core Ultra's, for one) is a
//! compute-only adapter: it does machine learning work and no graphics. Its
//! engines appear in the same `GPU Engine` counters as a GPU's, under its
//! own adapter id (LUID), so the id is what tells the two apart.

use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::DXCore::{
    DXCORE_ADAPTER_ATTRIBUTE_D3D12_CORE_COMPUTE, DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS,
    DXCoreCreateAdapterFactory, IDXCoreAdapter, IDXCoreAdapterFactory, IDXCoreAdapterList,
    InstanceLuid, IsHardware,
};
use windows::core::GUID;

/// `DXCORE_ADAPTER_ATTRIBUTE_D3D12_GENERIC_ML` (dxcore_interface.h), newer
/// than these bindings.
const GENERIC_ML: GUID = GUID::from_u128(0xb71b_0d41_1088_422f_a27c_0250_b7d3_a988);

/// The adapter ids of this computer's hardware NPUs, as the `GPU Engine`
/// counters spell them (`0x<high>_0x<low>`). Empty when there is none or
/// DXCore can't say (an older Windows lists no machine-learning adapters).
pub(crate) fn npu_luids() -> Vec<String> {
    // SAFETY: a plain factory request; the result is an owned interface.
    let Ok(factory) = (unsafe { DXCoreCreateAdapterFactory::<IDXCoreAdapterFactory>() }) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for filter in [GENERIC_ML, DXCORE_ADAPTER_ATTRIBUTE_D3D12_CORE_COMPUTE] {
        // SAFETY: a valid factory; the filter slice outlives the call.
        let Ok(list) = (unsafe { factory.CreateAdapterList::<IDXCoreAdapterList>(&[filter]) })
        else {
            continue;
        };
        // SAFETY: a valid list.
        for index in 0..unsafe { list.GetAdapterCount() } {
            // SAFETY: an index below the count.
            let Ok(adapter) = (unsafe { list.GetAdapter::<IDXCoreAdapter>(index) }) else {
                continue;
            };
            if let Some(id) = npu_id(&adapter).filter(|id| !ids.contains(id)) {
                ids.push(id);
            }
        }
    }
    ids
}

/// The adapter's id when it is a hardware adapter without graphics.
fn npu_id(adapter: &IDXCoreAdapter) -> Option<String> {
    // SAFETY: a valid adapter; the attribute GUID is a static.
    let graphics =
        unsafe { adapter.IsAttributeSupported(&DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS) };
    let mut hardware = 0_u8;
    // SAFETY: `IsHardware` is a one-byte boolean; the buffer is that size
    // and a live local.
    let read = unsafe {
        adapter.GetProperty(
            IsHardware,
            std::mem::size_of_val(&hardware),
            std::ptr::from_mut(&mut hardware).cast(),
        )
    };
    if graphics || read.is_err() || hardware == 0 {
        return None;
    }
    let mut luid = LUID::default();
    // SAFETY: `InstanceLuid` is a LUID; the buffer is that size and a live
    // local.
    unsafe {
        adapter.GetProperty(
            InstanceLuid,
            std::mem::size_of_val(&luid),
            std::ptr::from_mut(&mut luid).cast(),
        )
    }
    .ok()?;
    Some(super::super::accelerator_usage::luid_name(
        u32::from_ne_bytes(luid.HighPart.to_ne_bytes()),
        luid.LowPart,
    ))
}
