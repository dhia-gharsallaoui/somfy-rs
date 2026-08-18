#!/usr/bin/env python3
"""Print `.stack_sizes` entries whose demangled name matches a pattern.

The half of `crate::heap`'s stack-chain recipe that is otherwise done by hand.
`readelf -x .stack_sizes` prints a hex dump — a four-byte address then a
ULEB128 size, one entry per function — which is not something to read with an
eye, and the constants it feeds are the ones this project has watched go stale
three times.

    RUSTFLAGS="-Zemit-stack-sizes -C link-arg=-Tlinkall.x" \\
      cargo build --release --features chip-s3 \\
        --target xtensa-esp32s3-none-elf --bin firmware
    python3 stacksizes.py \\
      target/xtensa-esp32s3-none-elf/release/firmware firmware::restore

It prints frame sizes, largest first. It does **not** walk the call graph: which
frames sit on which chain is still a question for `objdump -d`, and on Xtensa
that is less obvious than it sounds — a far call is an `l32r` of the target into
a register followed by `callx8`, so the callee's name appears as a resolved
literal rather than as a branch target.

`nm`/`readelf` are taken from `PATH`, so `source ~/export-esp.sh` first for an
Xtensa image. For a RISC-V one, pass the tool prefix as the third argument.
"""

import subprocess
import sys

if len(sys.argv) < 2:
    sys.exit(__doc__)

elf = sys.argv[1]
pattern = sys.argv[2] if len(sys.argv) > 2 else ""
prefix = sys.argv[3] if len(sys.argv) > 3 else "xtensa-esp-elf-"


def tool(name, *args):
    return subprocess.run(
        [prefix + name, *args], capture_output=True, text=True, check=True
    ).stdout


# The section is a hex dump: sixteen bytes per line, in four space-separated
# groups, starting at a fixed column.
data = bytearray()
for line in tool("readelf", "-x", ".stack_sizes", elf).splitlines():
    if not line.startswith("  0x"):
        continue
    for chunk in line[13 : 13 + 35].split():
        data += bytes.fromhex(chunk)

names = {}
for line in tool("nm", "-C", elf).splitlines():
    parts = line.split(" ", 2)
    if len(parts) == 3 and parts[0].strip():
        try:
            names[int(parts[0], 16)] = parts[2]
        except ValueError:
            pass

frames = []
at = 0
while at + 4 <= len(data):
    address = int.from_bytes(data[at : at + 4], "little")
    at += 4
    size = 0
    shift = 0
    while at < len(data):
        byte = data[at]
        at += 1
        size |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            break
    name = names.get(address, f"<{address:#x}>")
    if pattern in name:
        frames.append((size, name))

for size, name in sorted(frames, reverse=True):
    print(f"{size:8d}  {name[:160]}")
