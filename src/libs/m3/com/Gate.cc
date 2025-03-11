/*
 * Copyright (C) 2015-2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2022 Nils Asmussen, Barkhausen Institut
 *
 * This file is part of M3 (Microkernel-based SysteM for Heterogeneous Manycores).
 *
 * M3 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 2 as
 * published by the Free Software Foundation.
 *
 * M3 is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 * General Public License version 2 for more details.
 */

#include <m3/Syscalls.h>
#include <m3/com/Gate.h>
#include <m3/tiles/OwnActivity.h>

namespace m3 {

Gate::~Gate() {
    release_ep();
}

EP *Gate::activate(capsel_t sel, bool mem) {
    auto ep = EPMng::get().acquire();
    activate_on(sel, *ep, mem);
    if(TCU::get().is_frozen(ep->id())) {
        auto err = TCU::get().unfreeze(ep->id());
        if(err != Errors::SUCCESS)
            throw MessageException("Unfreezing EP failed", err);
    }
    return ep;
}

void Gate::activate_on(capsel_t sel, const EP &ep, bool mem) {
    if(mem)
        Syscalls::activate_mgate(ep.sel(), sel);
    else
        Syscalls::activate_sgate(ep.sel(), sel);
}

void Gate::activate_rgate_on(capsel_t sel, const EP &ep, uintptr_t rbuf_virt, capsel_t rbuf_mem,
                             goff_t rbuf_off, size_t size) {
    Syscalls::activate_rgate(ep.sel(), sel, rbuf_mem, rbuf_off);
    if(rbuf_virt && TCU::get().is_frozen(ep.id())) {
        word_t phys;
        if(TMIF::translate(rbuf_virt, &phys) != Errors::SUCCESS)
            throw MessageException("Receive-buffer not mapped!?", Errors::INV_STATE);

        // check if the physical address and the buffer size is as expected (otherwise the kernel
        // could send us messages to overwrite specific areas of memory).
        auto rinfo = TCU::get().recv_info(ep.id()).unwrap();
        if(std::get<0>(rinfo) != phys)
            throw MessageException("Unexpected receive-buffer address", Errors::KERNEL_BROKEN);
        size_t rsize = static_cast<size_t>(1) << (std::get<1>(rinfo) + std::get<2>(rinfo));
        if(rsize != size)
            throw MessageException("Unexpected receive-buffer size", Errors::KERNEL_BROKEN);

        // check that the reply EPs are at the expected position (otherwise the kernel could let the
        // TCU overwrite other send EPs and thereby trick us to send to unexpected receivers).
        if(std::get<3>(rinfo) != ep.id() + 1)
            throw MessageException("Unexpected reply-EP offset", Errors::KERNEL_BROKEN);

        auto err = TCU::get().unfreeze(ep.id());
        if(err != Errors::SUCCESS)
            throw MessageException("Unfreezing EP failed", err);
    }
}

void Gate::release_ep(bool force_inval) noexcept {
    if(_ep) {
        EPMng::get().release(_ep, force_inval || (flags() & KEEP_CAP));
        _ep = nullptr;
    }
}

}
