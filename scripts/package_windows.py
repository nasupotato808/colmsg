"""Package an existing Windows x64 build: python scripts/package_windows.py."""
import argparse
import hashlib
import os
from pathlib import Path
import struct
import zipfile

ROOT = Path(__file__).resolve().parents[1]

README = """colmsg for Windows x64 - portable edition

1. Extract all files from this ZIP into a folder.
2. Double-click colmsg-ui.cmd.
3. Paste your current access token (the token value only).
4. Click Load members and select a member, or enter a group ID.
5. Choose a download folder and click Run.

Leave From and To unchecked to crawl all available messages up to date.
The window stays open after completion. Use Stop to cancel a running crawl.
The default API settings are for Sakurazaka46.

Requirements: Windows 10/11 x64 with the built-in Windows PowerShell 5.1.
Rust, Python, Visual Studio, and C++ Build Tools are not needed to run this package.
Keep colmsg.exe and vcruntime140.dll in the same folder as colmsg-ui.cmd.
Access tokens expire; paste a fresh token when authentication fails.

CLI usage: open a terminal in this folder and run colmsg.exe --help.

This build includes continuation-token pagination and the UI output handling fix.
"""


def x64_pe(path):
    with path.open("rb") as file:
        if file.read(2) != b"MZ":
            raise ValueError(f"Not a Windows executable: {path}")
        file.seek(0x3c)
        offset = struct.unpack("<I", file.read(4))[0]
        file.seek(offset)
        if file.read(4) != b"PE\0\0" or struct.unpack("<H", file.read(2))[0] != 0x8664:
            raise ValueError(f"Not a Windows x64 binary: {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, help="Path to the rebuilt colmsg.exe")
    parser.add_argument("--runtime", type=Path, help="x64 vcruntime140.dll from the MSVC Redist folder")
    args = parser.parse_args()
    release = ROOT / "target/release/colmsg.exe"
    exe = args.exe or (release if release.is_file() else ROOT / "colmsg.exe")
    if not exe.is_file():
        raise SystemExit("Build colmsg.exe first with cargo build --release.")
    sources = list((ROOT / "src").rglob("*.rs")) + [ROOT / "Cargo.toml"]
    if max(path.stat().st_mtime for path in sources) > exe.stat().st_mtime:
        raise SystemExit("The executable is older than the source. Rebuild it before packaging.")
    runtime = args.runtime
    if runtime is None:
        visual_studio = Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Microsoft Visual Studio"
        candidates = list(visual_studio.glob("*/*/VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT/vcruntime140.dll"))
        if candidates:
            runtime = max(candidates, key=lambda path: path.stat().st_mtime)
    if runtime is None or not runtime.is_file():
        raise SystemExit("Use --runtime to specify the x64 vcruntime140.dll from MSVC Redist.")
    x64_pe(exe)
    x64_pe(runtime)
    output = ROOT / "dist/colmsg-windows-x64.zip"
    output.parent.mkdir(exist_ok=True)
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        for path, name in ((exe, "colmsg.exe"), (runtime, "vcruntime140.dll"),
                (ROOT / "colmsg-ui.cmd", "colmsg-ui.cmd"), (ROOT / "LICENSE.txt", "LICENSE.txt")):
            archive.write(path, name)
        archive.writestr("README.txt", README.replace("\n", "\r\n"))
        archive.writestr("RUNTIME-NOTICE.txt", "Microsoft Visual C++ Runtime (vcruntime140.dll)\r\n"
            "Provided from the Microsoft Visual Studio MSVC x64 Redist directory.\r\n"
            "Microsoft copyright and license terms apply to this runtime component.\r\n"
            "The colmsg MIT license does not apply to this Microsoft component.\r\n")
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    checksum = output.with_suffix(output.suffix + ".sha256")
    checksum.write_text(f"{digest}  {output.name}\n", encoding="ascii")
    print(f"Created {output} ({output.stat().st_size / 1024**2:.2f} MiB)")
    print(f"SHA256: {digest}")


if __name__ == "__main__":
    main()
