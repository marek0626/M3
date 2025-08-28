import fcntl
import os
import re
import shutil
import subprocess

from pathlib import Path
from utils import die, run, which, parse_size, xml_xpath, xml_attr_value


class M3Lx:
    def __init__(self, platform):
        self.platform = platform
        self.enabled = self._have_initrds()

    def build(self):
        """Generates dependencies for M³Linux."""
        if not self.enabled:
            return

        # lock file (flock)
        lock_file = Path("build/.initrd.lock")
        lock_fd = lock_file.open("a")
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX)
        except Exception as e:
            die(f"Unable to acquire lock for initrd generation: {e}")

        try:
            crossroot = (self.platform.crossdir / "../../").resolve()
            bbl = (Path("build") / "riscv-pk" / "bbl").resolve()
            initrd = crossroot / "images" / "rootfs.cpio"

            # generate DTS and DTB
            initrd_size = self._create_initrd(initrd)
            self._generate_dtb(initrd_size)

            # copy initrds and bbls to module directory
            self._copy_modules(".//dom[@initrd]/@initrd", initrd)
            self._copy_modules(".//dom[@mux]/@mux", bbl)
        finally:
            # release lock (file closed on process exit)
            lock_fd.close()

    def _create_initrd(self, initrd: Path) -> int:
        """Generates the initrd image."""
        crossroot = (self.platform.crossdir / "../../").resolve()
        targetdir = crossroot / "build" / "buildroot-fs" / "cpio" / "target"
        fakeroot = crossroot / "build" / "buildroot-fs" / "cpio" / "fakeroot"
        if not fakeroot.is_file():
            die("Please run ./b mkrootfs first")

        # rsync source tree
        run(
            which("rsync"),
            "-auH",
            "--exclude=/THIS_IS_NOT_YOUR_ROOT_FILESYSTEM",
            f"{str(crossroot / "target")}/",
            str(targetdir),
        )

        # copy stripped binaries
        for f in (self.platform.builddir / "lxbin").glob("*"):
            strip = self.platform.crossdir / f"{self.platform.crossname}strip"
            run(str(strip), "-o", str(targetdir / f.name), str(f))

        # overlay files
        for item in (Path("src") / "m3lx" / "rootfs").iterdir():
            if item.is_file():
                shutil.copy(item, targetdir / item.name)
            elif item.is_dir():
                shutil.copytree(item, targetdir / item.name, dirs_exist_ok=True)
            else:
                print(f"Skipping item '{item}' in root FS")

        # fakeroot to create the cpio image
        fakeroot_env = os.environ.copy()
        fakeroot_env["PATH"] = f"{crossroot}/host/sbin:{fakeroot_env['PATH']}"
        fakeroot_env["FAKEROOTDONTTRYCHOWN"] = "1"
        run(
            f"{crossroot}/host/bin/fakeroot", "--", fakeroot,
            cwd="cross/buildroot",
            capture=subprocess.DEVNULL,
            env=fakeroot_env,
        )

        # determine size (rounded up to pages)
        size = int(
            run(which("stat"), "--printf=%s", str(initrd), capture=subprocess.PIPE).stdout.strip()
        )
        size = (size + 0xFFF) & 0xFFFFF000
        return size

    def _generate_dtb(self, initrd_size):
        dtb_names = set()
        dtb_tags = xml_xpath(self.platform.cfg, ".//dom[@dtb]/@dtb")
        for line in dtb_tags.split("\n"):
            dtb_names.add(xml_attr_value(line))
        for dtb in set(dtb_names):
            # verify that all uses of this dtb have the same muxmem attribute
            mem_cnt = int(xml_xpath(self.platform.cfg, f'count(.//dom[@dtb="{dtb}"]/@muxmem)'))
            if mem_cnt != 1:
                die(f'DTB "{dtb}" is used with different memory sizes (muxmem).')

            # determine memory size
            mem_str = xml_xpath(self.platform.cfg, f'string(.//dom[@dtb="{dtb}"]/@muxmem)')
            mem_size = parse_size(mem_str)
            if mem_size & (mem_size - 1):
                die(f"The memory size ({mem_size}) for Linux needs to be a power of two!")

            # create a temporary DTS with the right values
            mem_off = 0x10000000
            initrd_end = mem_off + mem_size
            initrd_start = initrd_end - initrd_size
            new_dts = self._generate_dts(dtb, initrd_start, initrd_end, mem_off, mem_size)

            # compile to DTB
            dtb_path = xml_xpath(self.platform.cfg, f'string(.//mods/mod[@name="{dtb}"]/@file)')
            dtb_dst = self.platform.moddir / dtb_path
            run(which("dtc"), "-O", "dtb", str(new_dts), "-o", str(dtb_dst))

    def _generate_dts(self, dtb: str, initrd_start: int, initrd_end: int,
                      mem_off: int, mem_size: int):
        """Generates DTS with given initrd settings and memory size."""
        src_dts = Path(f"src/m3lx/configs/{self.platform.target}.dts")
        dst_dts = Path(f"{self.platform.outdir}/{dtb}.dts")
        dts = src_dts.read_text()
        dts = re.sub(
            r'linux,initrd-start = <.*>;',
            f'linux,initrd-start = <{hex(initrd_start)}>;',
            dts
        )
        dts = re.sub(
            r'linux,initrd-end = <.*>;',
            f'linux,initrd-end = <{hex(initrd_end)}>;',
            dts
        )
        dts = re.sub(
            r'reg = <MEM_REGION>;',
            f'reg = <0x00000000 {hex(mem_off)} 0x00000000 {hex(mem_size)}>;',
            dts,
        )
        # TODO get rid of that env variable here
        if os.getenv("M3_HW_UARTNOBUF") == "1":
            dts = dts.replace(
                'compatible = "sifive,uart0";',
                'compatible = "sifive,uart0"; nobuf = "1";',
            )
        dst_dts.write_text(dts)
        return dst_dts

    def _copy_modules(self, xpath: str, src_path: Path):
        """Copies the modules identified by `xpath` to the modules directory."""
        # determine modules to copy
        names = set()
        res = xml_xpath(self.platform.cfg, xpath)
        for line in res.split("\n"):
            names.add(xml_attr_value(line))

        # copy them to module directory
        for name in names:
            mod_name = xml_xpath(self.platform.cfg, f'string(.//mods/mod[@name="{name}"]/@file)')
            dst = self.platform.moddir / mod_name
            shutil.copy2(src_path, dst)

    def _have_initrds(self) -> bool:
        """Returns true if there are any <dom initrd=... occurrences."""
        initrd_cnt = int(xml_xpath(self.platform.cfg, "count(.//dom[@initrd])"))
        return initrd_cnt > 0
