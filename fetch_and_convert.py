#!/usr/bin/env python3

import argparse
import re
import shlex
import shutil
import signal
import subprocess
import sys
import textwrap
import time
from pathlib import Path

import requests
from bs4 import BeautifulSoup

###############################################################################
# Configuration
###############################################################################

BASE_URL       = "https://data.lczero.org/files/training_data/test91/"
PARSER_BIN     = Path("./target/release/lc0v6-binpack-converter")
TIMESTAMP_RE   = re.compile(r"(\d{8}-\d{4})")
SPINNER_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

HEADERS = {"User-Agent": "Mozilla/5.0 (compatible; lc0-fetcher/1.0)"}

###############################################################################
# Globals
###############################################################################

current_proc: subprocess.Popen | None = None
stop_requested = False

###############################################################################
# Signal handling
###############################################################################

def handle_signal(signum, frame):
    global stop_requested, current_proc
    stop_requested = True
    print("\n🛑 Interrupt received")
    if current_proc and current_proc.poll() is None:
        print("⚡ Stopping active process...")
        current_proc.terminate()
        try:
            current_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            current_proc.kill()
        current_proc = None

signal.signal(signal.SIGINT,  handle_signal)
signal.signal(signal.SIGTERM, handle_signal)

###############################################################################
# Helpers
###############################################################################

def banner(msg: str):
    line = "═" * 47
    print(f"\n{line}\n{msg}\n{line}")


def run(cmd: list[str]) -> int:
    global current_proc
    current_proc = subprocess.Popen(cmd)
    ret = current_proc.wait()
    current_proc = None
    return ret


def run_with_spinner(cmd: list[str], label: str) -> int:
    global current_proc
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    current_proc = proc

    i = 0
    while proc.poll() is None:
        frame = SPINNER_FRAMES[i % len(SPINNER_FRAMES)]
        print(f"\r{frame} {label}", end="", flush=True)
        i += 1
        time.sleep(0.15)

    ret = proc.wait()
    current_proc = None
    print(f"\r{'✅' if ret == 0 else '❌'} {label}" + " " * 10)
    return ret


def timestamp_of(filename: str) -> str | None:
    m = TIMESTAMP_RE.search(filename)
    return m.group(1) if m else None


###############################################################################
# Argument parsing
###############################################################################

parser = argparse.ArgumentParser(
    formatter_class=argparse.RawDescriptionHelpFormatter,
    description="Download lc0 Test91 training data and convert it into binpacks.",
    epilog=textwrap.dedent("""
        Examples:
          Process a range (--from is the older timestamp, --to is the newer one):
            %(prog)s \\
                --from 20251106-1117 \\
                --to   20251107-0917

          Process from a point onward:
            %(prog)s \\
                --from 20251106-1117

          Process up to a point:
            %(prog)s \\
                --to 20251107-0917

          With Syzygy tablebases:
            %(prog)s \\
                --from 20251106-1117 \\
                --syzygy-path /path/to/syzygy

          With backward propagation limit:
            %(prog)s \\
                --from 20251106-1117 \\
                --syzygy-path /path/to/syzygy \\
                --backpropagate-limit 10
    """),
)
parser.add_argument("--from", dest="from_ts", metavar="TIMESTAMP",
                    help="Start timestamp, e.g. 20251106-1117 (inclusive)")
parser.add_argument("--to", dest="to_ts", metavar="TIMESTAMP",
                    help="End timestamp, e.g. 20251107-0917 (inclusive)")
parser.add_argument("--syzygy-path", metavar="PATHS",
                    help="Colon-separated Syzygy tablebase directories (passed through to parser)")
parser.add_argument("--backpropagate", action="store_true",
                    help="Propagate first Syzygy hit backward to move 1 (requires --syzygy-path)")
parser.add_argument("--backpropagate-limit", type=int, metavar="N",
                    help="Like --backpropagate but only patches N plies before the hit (implies --backpropagate)")
args = parser.parse_args()

if not args.from_ts and not args.to_ts:
    parser.print_help()
    sys.exit(2)

###############################################################################
# Sanity checks
###############################################################################

if not PARSER_BIN.exists():
    print(f"❌ Parser binary not found: {PARSER_BIN}")
    sys.exit(1)

if args.backpropagate and not args.syzygy_path:
    print("❌ --backpropagate requires --syzygy-path")
    sys.exit(1)

if args.backpropagate_limit and not args.syzygy_path:
    print("❌ --backpropagate-limit requires --syzygy-path")
    sys.exit(1)

###############################################################################
# Build reusable parser flags
###############################################################################

def build_parser_flags() -> list[str]:
    flags = ["--summary"]
    if args.syzygy_path:
        flags += ["--syzygy-path", args.syzygy_path]
    if args.backpropagate_limit is not None:
        flags += ["--backpropagate-limit", str(args.backpropagate_limit)]
    elif args.backpropagate:
        flags.append("--backpropagate")
    return flags

###############################################################################
# Fetch tarball list
###############################################################################

banner("🌐 Fetching Test91 tarball list")

resp = requests.get(BASE_URL, headers=HEADERS, timeout=30)
resp.raise_for_status()

soup = BeautifulSoup(resp.text, "html.parser")
tarballs = sorted(
    (a["href"] for a in soup.find_all("a", href=re.compile(r"\.tar$")))
)

if not tarballs:
    print("❌ No tarballs found.")
    sys.exit(1)

###############################################################################
# Validate range boundaries
###############################################################################

timestamps = [timestamp_of(t) for t in tarballs]

if args.from_ts and args.from_ts not in timestamps:
    print(f"❌ --from timestamp not found:\n   {args.from_ts}")
    sys.exit(1)

if args.to_ts and args.to_ts not in timestamps:
    print(f"❌ --to timestamp not found:\n   {args.to_ts}")
    sys.exit(1)

###############################################################################
# Range filtering
###############################################################################

if args.from_ts or args.to_ts:
    start = timestamps.index(args.from_ts) if args.from_ts else 0
    end   = timestamps.index(args.to_ts)   if args.to_ts   else len(tarballs) - 1
    tarballs = tarballs[start:end + 1]

###############################################################################
# Summary
###############################################################################

banner("📋 Processing Summary")
print(f"📦 Tarballs selected : {len(tarballs)}")
if args.from_ts:
    print(f"📍 From              : {args.from_ts}")
if args.to_ts:
    print(f"🏁 To                : {args.to_ts}")
if args.syzygy_path:
    print(f"♟️  Syzygy path       : {args.syzygy_path}")
if args.backpropagate_limit is not None:
    print(f"⏪ Backprop limit    : {args.backpropagate_limit}")
elif args.backpropagate:
    print(f"⏪ Backpropagate     : full")

if not tarballs:
    print("\n❌ Selected range contains no tarballs.")
    sys.exit(1)

###############################################################################
# Main loop
###############################################################################

parser_flags = build_parser_flags()

for tarball in tarballs:

    if stop_requested:
        print("\n🛑 Stopping due to interrupt.")
        sys.exit(130)

    name         = tarball.removesuffix(".tar")
    url          = f"{BASE_URL}{tarball}"
    extract_path = Path(name)
    binpack_path = Path(f"{name}.binpack")

    banner(f"🚂 Processing {tarball}")

    # Skip
    if binpack_path.exists():
        print("⏭️  Binpack already exists, skipping.")
        continue

    # Download + extract (streamed, no tarball saved to disk)
    if not extract_path.exists():
        # Check Content-Length to detect empty tarballs (10240 bytes) before downloading
        head = requests.head(url, headers=HEADERS, timeout=30)
        content_length = int(head.headers.get("Content-Length", -1))
        if content_length == 10240:
            print("⏭️  Tarball is empty (10240 bytes), skipping.")
            continue

        cmd = f"set -o pipefail; wget -qO- {shlex.quote(url)} | tar -xf -"
        ret = run_with_spinner(["bash", "-c", cmd], f"Downloading {tarball}")
        if ret != 0:
            print(f"❌ Download/extract failed (exit {ret})")
            sys.exit(ret)
    else:
        print("📂 Already extracted.")

    # Verify the directory has .gz files before invoking the parser
    gz_files = list(extract_path.glob("*.gz"))
    if not gz_files:
        print("⚠️  No .gz files found, nothing to parse.")
        shutil.rmtree(extract_path, ignore_errors=True)
        continue

    print(f"🧠 Parsing {len(gz_files)} files via -d flag...")

    ret = run([
        str(PARSER_BIN),
        *parser_flags,
        "-o", str(binpack_path),
        "-d", str(extract_path),
    ])
    if ret != 0:
        print(f"❌ Parser failed (exit {ret})")
        sys.exit(ret)

    # Cleanup
    print("🧹 Cleaning up...")
    shutil.rmtree(extract_path, ignore_errors=True)

    print(f"✅ Finished {tarball}")

###############################################################################
# Done
###############################################################################

banner("🎉🎉🎉 ALL DONE! 🎉🎉🎉")
print("🧠 Parsing complete")
print("📦 Binpacks generated")
print("🧹 Workspace cleaned\n")