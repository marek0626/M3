
/// A TCU register
pub type Reg = u64;
/// An endpoint id
pub type EpId = u16;

pub const PMEM_PROT_EPS: usize = 4;
/// The send EP for kernel calls from TileMux
pub const KPEX_SEP: EpId = PMEM_PROT_EPS as EpId + 0;
/// The receive EP for kernel calls from TileMux
pub const KPEX_REP: EpId = PMEM_PROT_EPS as EpId + 1;

pub const PAGE_BITS: usize = 12;
pub const PAGE_SIZE: usize = 1 << PAGE_BITS;
/// The base address of the TCU's MMIO area
pub const MMIO_ADDR: usize = 0xF000_0000;
/// The size of the TCU's MMIO area
pub const MMIO_SIZE: usize = PAGE_SIZE * 2;

pub const UNPRIV_REGS_START: usize = 2;

int_enum! {
    /// The unprivileged registers
    #[allow(dead_code)]
    pub struct UnprivReg : Reg {
        /// Starts commands and signals their completion
        const COMMAND       = 0x0;
        /// Specifies the data address and size
        const DATA          = 0x1;
        /// Specifies an additional argument
        const ARG1          = 0x2;
        /// The current time in nanoseconds
        const CUR_TIME      = 0x3;
        /// Prints a line into the gem5 log
        const PRINT         = 0x4;
    }
}

int_enum! {
    /// The commands
    pub struct CmdOpCode : u64 {
        /// The idle command has no effect
        const IDLE          = 0x0;
        /// Sends a message
        const SEND          = 0x1;
        /// Replies to a message
        const REPLY         = 0x2;
        /// Reads from external memory
        const READ          = 0x3;
        /// Writes to external memory
        const WRITE         = 0x4;
        /// Fetches a message
        const FETCH_MSG     = 0x5;
        /// Acknowledges a message
        const ACK_MSG       = 0x6;
        /// Puts the CU to sleep
        const SLEEP         = 0x7;
    }
}