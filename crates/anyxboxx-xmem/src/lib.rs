//! xmem.dll in Rust: the console's memory allocator, translated to the PC's memory. Port of
//! `runtime/xmem` (C++), same 16 exports and same behaviour; checked by running
//! `tests/test_xmem_era.py`, `tests/test_xmem_virtual.py` and the xmem tests of
//! `tests/test_runtime_cpp.py` against this DLL (`tests/test_rust_dlls.py`).
//!
//! Signatures read at the calls (no public header): XMemAlloc(size, attributes) and
//! XMemFree(pointer, attributes), routed to hooks the title may replace (XMemSetAllocationHooks);
//! XMemVirtualQuery(process, address, 64-byte buffer). The XALLOC_ATTRIBUTES are split with the
//! ERA layout (VIctory's `EraFields`): heap, graphics and physical types are served.
//!
//! Each block goes back to the allocator that made it: default blocks are recorded with their
//! placement, and both hooks are published together (one atomic pair). Graphics blocks are also
//! registered for XMemVirtualQuery.
//!
//! What lives in VIctory (`victory_hle::modules::xmem`, decision D3): the ERA field split
//! (`EraFields`, also behind VIctory's own `XMemAlloc` alignment) and `XMemIsToolsMemAvailable`.
//! The rest keeps the C++ DLL's behaviour, which differs from VIctory's on purpose, because the
//! pytest oracle checks it and VIctory's versions depend on VIctory's runtime:
//! - XMemAlloc hooks: here set by XMemSetAllocationHooks; VIctory reads the title exe's
//!   `__XMemAllocationHooks__` export through its loader.
//! - Attributes: here bit 33 and up (ERA reserved) are refused with ERROR_INVALID_PARAMETER;
//!   VIctory reads bit 33 as "a 32-bit address" and serves it below 4 GiB.
//! - XMemVirtualAlloc: here a plain VirtualAlloc with checked flags; VIctory places regions
//!   between 4 GiB and 2^40, uses placeholders for 64 KiB-page reservations and reports them to
//!   its GPU side.
//! - XMemVirtualQuery: here byte +0x30 bit 0 marks registered graphics regions only; VIctory
//!   writes a u64 flags word (3, graphics) for every committed page.
//! - Physical pages: here identifiers without aliasing; VIctory aliases through file sections
//!   but maps only into placeholder reservations (the oracle maps into plain ones).
//! - XMemGetWorkingSetStatistics and XMemTransferMemory: here refused (ERROR_NOT_SUPPORTED);
//!   VIctory answers from its console budget (`victory_gpu::console`).
//!
//! The ERA heap and XMemVirtualQuery are adapted from Xbox2Windows (`src/era_heap.*`,
//! `src/era_query.*`, https://github.com/R0Y00/Xbox2Windows), under the MIT License:
//!
//!   MIT License
//!
//!   Copyright (c) 2026 Xbox2Windows contributors
//!
//!   Permission is hereby granted, free of charge, to any person obtaining a copy
//!   of this software and associated documentation files (the "Software"), to deal
//!   in the Software without restriction, including without limitation the rights
//!   to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//!   copies of the Software, and to permit persons to whom the Software is
//!   furnished to do so, subject to the following conditions:
//!
//!   The above copyright notice and this permission notice shall be included in all
//!   copies or substantial portions of the Software.
//!
//!   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//!   IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//!   FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//!   AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//!   LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//!   OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//!   SOFTWARE.
#![allow(non_snake_case)]

mod era;
mod era_query;
mod heap;
mod log;
mod pages;
mod physical;
mod virtual_alloc;

#[cfg(test)]
mod tests;

pub use heap::{
    AllocRoutine, AnyXboxX_DecodeAllocAttributes, FreeRoutine, XMemAlloc, XMemAllocDefault,
    XMemFree, XMemFreeDefault, XMemGetAllocationHooks, XMemGetWorkingSetStatistics,
    XMemSetAllocationHooks, XMemVirtualQuery,
};
pub use physical::{
    XMemAllocatePhysicalPages, XMemFreePhysicalPages, XMemIsToolsMemAvailable,
    XMemMapPhysicalPages, XMemMapPhysicalPagesScatter, XMemTransferMemory,
};
pub use victory_hle::modules::xmem::EraFields;
pub use virtual_alloc::XMemVirtualAlloc;
