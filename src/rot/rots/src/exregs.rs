use core::cmp::min;

use base::cell::{StaticCell, StaticRefCell};
use base::errors::{Code, Error};
use base::io::LogFlags;
use base::kif::Perm;
use base::mem::GlobOff;
use base::tcu::{self, GenId, TileId, TCU};
use base::util::math;
use base::vec::Vec;
use base::{env, log, vec};

#[derive(Debug)]
struct ExReg {
    mtile: TileId,
    idx: usize,
    utile: TileId,
    ugen: GenId,
    addr: GlobOff,
    size: GlobOff,
    locked: bool,
}

static REGS: StaticRefCell<Vec<ExReg>> = StaticRefCell::new(vec![]);
static BUF: StaticRefCell<[u8; 1024]> = StaticRefCell::new([0u8; 1024]);
static ZEROING: StaticCell<bool> = StaticCell::new(true);

pub fn disable_zeroing() {
    ZEROING.set(false);
}

#[allow(clippy::too_many_arguments, clippy::absurd_extreme_comparisons)]
pub fn add(
    mtile: TileId,
    idx: usize,
    utile: TileId,
    ugen: GenId,
    addr: GlobOff,
    size: GlobOff,
    perm: Perm,
    locked: bool,
) -> Result<(), Error> {
    if idx >= tcu::EXREG_REGS {
        return Err(Error::new(Code::OutOfBounds));
    }

    let mut regs = REGS.borrow_mut();
    for reg in regs.iter_mut().filter(|r| r.mtile == mtile) {
        if reg.idx == idx {
            log!(LogFlags::RoTExRegs, "[{}].{}: already exists", mtile, idx);
            return Err(Error::new(Code::Exists));
        }

        if math::overlaps(addr, addr + size, reg.addr, reg.addr + reg.size) {
            if addr != reg.addr || size != reg.size {
                log!(
                    LogFlags::RoTExRegs,
                    "[{}].{}: partial overlap is not supported",
                    mtile,
                    reg.idx
                );
                return Err(Error::new(Code::NotSup));
            }
            // allow overlaps with our own tile, because we can never get rid of those
            if reg.locked && reg.utile != env::boot().tile_id() {
                log!(
                    LogFlags::RoTExRegs,
                    "[{}].{}: overlaps and is locked",
                    mtile,
                    reg.idx
                );
                return Err(Error::new(Code::ExRegOverlaps));
            }
            if locked {
                reg.locked = true;
            }
        }
    }

    log!(
        LogFlags::RoTExRegs,
        "[{}].{} = [utile={}, ugen={}, perm={:?}, addr={:#x}, size={:#x}, locked={}]",
        mtile,
        idx,
        utile,
        ugen,
        perm,
        addr,
        size,
        locked
    );

    // write region to memory tile
    let idxmtile = rot::IndexedTile::new_from_env(mtile).unwrap();
    let (cfg, range) = TCU::build_exreg(utile, ugen, idx, addr, size, perm)
        .ok_or_else(|| Error::new(Code::InvArgs))?;
    idxmtile
        .write_tcu(&[cfg], TCU::exreg_addr(idx).as_goff())
        .unwrap();
    idxmtile
        .write_tcu(&[range], TCU::exreg_addr(idx).as_goff() + 8)
        .unwrap();

    regs.push(ExReg {
        mtile,
        idx,
        utile,
        ugen,
        addr,
        size,
        locked,
    });

    Ok(())
}

pub fn rem(mtile: TileId, idx: usize) -> Result<(), Error> {
    let mut regs = REGS.borrow_mut();
    let reg = regs
        .iter()
        .find(|r| r.mtile == mtile && r.idx == idx)
        .ok_or_else(|| Error::new(Code::NotFound))?;

    log!(LogFlags::RoTExRegs, "[{}].{} = invalid", mtile, idx,);

    // recreate the EP with the expected tile generation
    let idxutile = rot::IndexedTile::new_from_env(reg.utile).unwrap();
    idxutile.init(Perm::RW, reg.ugen + 1);
    TCU::unfreeze(idxutile.ep()).unwrap();

    // check user tile generation
    let utile_features: tcu::Reg = idxutile
        .read_tcu_obj(TCU::ext_reg_addr(tcu::ExtReg::Features).as_goff())
        .unwrap();
    let utile_gen = (utile_features >> 4) as tcu::GenId;
    // if the user tile has not been reset yet, we cannot get rid of its exclusive region
    if utile_gen != reg.ugen + 1 {
        return Err(Error::new(Code::InvState));
    }

    // are there no overlaps with this region left?
    let idxmtile = rot::IndexedTile::new_from_env(mtile).unwrap();
    if ZEROING.get()
        && !regs.iter().any(|r| {
            r.mtile == mtile
                && r.idx != idx
                && math::overlaps(r.addr, r.addr + r.size, reg.addr, reg.addr + reg.size)
        })
    {
        // clear the memory to erase any secrets
        clear_mem(idxmtile.ep(), reg.addr, reg.size).unwrap();
    }

    // make region invalid
    idxmtile
        .write_tcu(&[0u64, 0], TCU::exreg_addr(idx).as_goff())
        .unwrap();

    regs.retain(|r| r.mtile != mtile || r.idx != idx);
    Ok(())
}

fn clear_mem(ep: tcu::EpId, mut off: GlobOff, mut size: GlobOff) -> Result<(), Error> {
    while size > 0 {
        let len = min(size, BUF.borrow().len() as GlobOff);
        TCU::write(ep, BUF.borrow().as_ptr(), len as usize, off)?;
        off += len;
        size -= len;
    }
    Ok(())
}
