/*
 * Copyright (C) 2023 Nils Asmussen, Barkhausen Institut
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

#include <base/Env.h>
#include <base/Init.h>
#include <base/TCU.h>
#include <base/TileDesc.h>
#include <base/arch/linux/Init.h>
#include <base/arch/linux/MMap.h>

#include <fcntl.h>
#include <signal.h>
#include <sys/epoll.h>
#include <unistd.h>

namespace m3lx {

struct LinuxInit {
    LinuxInit();

    static int init_dev();
    static void init_env(int tcu_fd);

    int fd;
};

static INIT_PRIO_LXDEV LinuxInit lxdev;

int tcu_fd() {
    return lxdev.fd;
}

static void handle_sigsegv(int sig, siginfo_t *sig_info, void *ucontext) {
    (void)sig;
    (void)ucontext;

    if(sig_info == nullptr)
        _exit(1);

    void *addr = sig_info->si_addr;
    if(addr == nullptr)
        _exit(1);

    uintptr_t addr_int = reinterpret_cast<uintptr_t>(addr);
    if(addr_int >= m3::TCU::MMIO_ADDR && addr_int < (m3::TCU::MMIO_ADDR + m3::TCU::MMIO_SIZE))
        mmap_tcu(tcu_fd(), reinterpret_cast<void *>(m3::TCU::MMIO_ADDR), m3::TCU::MMIO_SIZE,
                 MemType::TCU, m3::KIF::Perm::RW);
    else
        _exit(1);
}

void install_sig_handler() {
    sigset_t mask;
    sigemptyset(&mask);

    struct sigaction new_action;
    memset(&new_action, 0, sizeof(new_action));
    new_action.sa_sigaction = handle_sigsegv;
    new_action.sa_mask = mask;
    new_action.sa_flags = SA_SIGINFO;

    struct sigaction old_action;

    sigaction(SIGSEGV, &new_action, &old_action);
    sigaction(SIGBUS, &new_action, &old_action);
}

LinuxInit::LinuxInit() : fd(init_dev()) {
    init_env(fd);
    install_sig_handler();
#if defined(__hw__) || defined(__gem5__)
    mmap_tcu(fd, reinterpret_cast<void *>(m3::TCU::MMIO_EPS_ADDR), m3::TCU::endpoints_size(),
             MemType::TCUEps, m3::KIF::Perm::R);
#endif

    auto [rbuf_virt_addr, rbuf_size] = m3::TileDesc(m3::bootenv()->tile_desc).rbuf_std_space();
    mmap_tcu(fd, reinterpret_cast<void *>(rbuf_virt_addr), rbuf_size, MemType::StdRecvBuf,
             m3::KIF::Perm::R);
}

int LinuxInit::init_dev() {
    int fd = open("/dev/tcu", O_RDWR | O_SYNC);
    assert(fd != -1);
    return fd;
}

void LinuxInit::init_env(int tcu_fd) {
    mmap_tcu(tcu_fd, reinterpret_cast<void *>(m3::bootenv()), ENV_SIZE, MemType::Environment,
             m3::KIF::Perm::RW);
}

}
