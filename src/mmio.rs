use vstd::prelude::*;

use crate::phy::TisPhy;
use crate::tis::TisErr;

verus! {

pub assume_specification [u32::from_le] (_0: u32) -> u32;
pub assume_specification [u32::to_le] (_0: u32) -> u32;
pub assume_specification [core::hint::spin_loop] ();

// ===========================================================================
// 圈层 B：可信规约的落地
// ===========================================================================
//
// core 层不再直接持有裸指针，也不依赖任何平台 MMIO 库。这里改成「安全后端」：
// 具体平台把寄存器读写能力实现成 `TisMmioBackend`，再交给本类型封装成 `TisPhy`。
// 这样 core 侧不出现不安全代码，平台细节完全留在圈层 C / HAL。

// ---------------------------------------------------------------------------
// 安全后端接口
// ---------------------------------------------------------------------------

pub trait TisMmioBackend {
    fn read8(&mut self, addr: u32) -> Result<u8, TisErr>;

    fn read32(&mut self, addr: u32) -> Result<u32, TisErr>;

    fn write8(&mut self, addr: u32, value: u8) -> Result<(), TisErr>;

    fn write32(&mut self, addr: u32, value: u32) -> Result<(), TisErr>;

    fn delay(&mut self);
}

// ---------------------------------------------------------------------------
// 寄存器窗口（后端适配）
// ---------------------------------------------------------------------------

pub struct TisMmio<B: TisMmioBackend> {
    backend: B,
    /// 每次 [`TisPhy::delay`] 空转的圈数。不是时长，真实间隔取决于目标 CPU 主频。
    spin: u32,
    /// 幽灵累积量，供 `phy.rs` 的规约引用。运行时零开销。
    ghost written: Seq<u8>,
}

impl<B: TisMmioBackend> TisMmio<B> {
    #[verifier::external_body]
    pub fn new(backend: B, spin: u32) -> (r: Self)
        ensures
            r.fifo_written() =~= Seq::<u8>::empty(),
    {
        TisMmio { backend, spin, written: Seq::empty() }
    }
}

impl<B: TisMmioBackend> TisPhy for TisMmio<B> {
    closed spec fn fifo_written(&self) -> Seq<u8> {
        self.written
    }

    // -----------------------------------------------------------------------
    // 寄存器
    // -----------------------------------------------------------------------

    #[verifier::external_body]
    fn read8(&mut self, addr: u32) -> (r: Result<u8, TisErr>) {
        self.backend.read8(addr)
    }

    #[verifier::external_body]
    fn read32(&mut self, addr: u32) -> (r: Result<u32, TisErr>) {
        self.backend.read32(addr).map(|v| u32::from_le(v))
    }

    #[verifier::external_body]
    fn write8(&mut self, addr: u32, value: u8) -> (r: Result<(), TisErr>) {
        self.backend.write8(addr, value)
    }

    #[verifier::external_body]
    fn write32(&mut self, addr: u32, value: u32) -> (r: Result<(), TisErr>) {
        self.backend.write32(addr, value.to_le())
    }

    // -----------------------------------------------------------------------
    // 数据口
    // -----------------------------------------------------------------------

    #[verifier::external_body]
    fn read_fifo(&mut self, addr: u32, out: &mut [u8], off: usize, n: usize) -> (r: Result<
        (),
        TisErr,
    >) {
        let out_len = out.len();
        let mut i = 0usize;
        while i < n
            invariant
                i <= n,
                off + i <= out_len,
                out_len == old(out).len(),
            decreases n - i,
        {
            let idx = off + i;
            if idx >= out_len {
                return Err(TisErr::Phy);
            }
            match self.backend.read8(addr) {
                Ok(b) => out[idx] = b,
                Err(e) => return Err(e),
            }
            proof {
                assert(off + (i + 1) <= out_len);
            }
            i += 1;
        }
        Ok(())
    }

    #[verifier::external_body]
    fn write_fifo(&mut self, addr: u32, data: &[u8], off: usize, n: usize) -> (r: Result<
        (),
        TisErr,
    >) {
        let data_len = data.len();
        let mut i = 0usize;
        while i < n
            invariant
                i <= n,
                off + i <= data_len,
            decreases n - i,
        {
            let idx = off + i;
            if idx >= data_len {
                return Err(TisErr::Phy);
            }
            let byte = match data.get(idx) {
                Some(v) => *v,
                None => return Err(TisErr::Phy),
            };
            match self.backend.write8(addr, byte) {
                Ok(()) => i += 1,
                Err(e) => {
                    proof {
                        self.written = self.written + data@.subrange(off as int, idx as int);
                    }
                    return Err(e);
                },
            }
            proof {
                assert(off + (i + 1) <= data_len);
            }
        }
        proof {
            self.written = self.written + data@.subrange(off as int, off + n as int);
        }
        Ok(())
    }

    /// 写入 `TPM_STS_COMMAND_READY`（0x40）。
    #[verifier::external_body]
    fn reset_fifo(&mut self, addr: u32) {
        let _ = self.backend.write8(addr, 0x40u8);
        proof {
            self.written = self.written.subrange(0, 0);
        }
    }

    // -----------------------------------------------------------------------
    // 节流
    // -----------------------------------------------------------------------

    #[verifier::external_body]
    fn delay(&mut self) {
        self.backend.delay();
    }
}

} // verus!
