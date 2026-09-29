#!/usr/bin/env python3
"""Record croft in a terminal as a GIF for a pull request description.

Drives the real binary inside a sized tmux session with a throwaway HOME, so
nothing touches your own config, and snapshots the screen after each step.
The frames are replayed into xterm.js in headless Chromium (render.mjs) with a
Nerd Font, so croft's icons draw, then written out as one GIF with an optional
PNG still. See "Show it" in CONTRIBUTING.md; examples/settings_editor.py is a
complete scenario.

Needs tmux, node, Pillow (`pip install pillow`) and a Chromium that
playwright-core can launch (`npx playwright install chromium`, or point
CROFT_DEMO_CHROMIUM at one). The first run installs render.mjs's two npm
packages and downloads the font into ~/.cache/croft-demo.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "croft-demo"
FONT_URL = "https://github.com/ryanoasis/nerd-fonts/releases/latest/download/DejaVuSansMono.tar.xz"
FONTS = ("DejaVuSansMNerdFontMono-Regular.ttf", "DejaVuSansMNerdFontMono-Bold.ttf")
SESSION = "croft-demo"


class Demo:
    """One recording: start it, script keys and snapshots, then `save`."""

    def __init__(
        self,
        binary: str | Path,
        cols: int = 150,
        rows: int = 36,
        user_config: dict | None = None,
        workspace_config: dict | None = None,
        files: dict[str, str] | None = None,
    ):
        self.binary = Path(binary).resolve()
        self.cols, self.rows = cols, rows
        # Under /tmp and short: croft's unix sockets live under the cache dir,
        # and an AF_UNIX path is capped near 104 bytes. The path also shows
        # in status messages ("saved to ..."), so short reads better.
        self.home = Path(tempfile.mkdtemp(prefix="cd-", dir="/tmp"))
        self.project = self.home / "proj"
        self.frames: list[tuple[str, int]] = []
        config = {"suppress_terminal_warning": True, **(user_config or {})}
        _write_json(self.home / ".config/croft/config.json", config)
        if workspace_config is not None:
            _write_json(self.project / ".croft/config.json", workspace_config)
        for rel, text in (files or {"README.md": "# demo\n"}).items():
            path = self.project / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)

    def start(self, settle: float = 4.0) -> None:
        subprocess.run(["tmux", "kill-session", "-t", SESSION], capture_output=True)
        env = {
            "HOME": str(self.home),
            "XDG_CACHE_HOME": str(self.home / ".cache"),
            "COLORTERM": "truecolor",
        }
        cmd = " ".join(f"{k}={_q(v)}" for k, v in env.items())
        _tmux(
            "-f", "/dev/null", "new-session", "-d", "-s", SESSION,
            "-x", str(self.cols), "-y", str(self.rows),
            "-e", "TERM=xterm-256color", "-e", "PS1=$ ",
            f"cd {_q(str(self.project))} && env -u XDG_CONFIG_HOME {cmd} {_q(str(self.binary))} .",
        )
        _tmux("set", "-as", "terminal-features", ",*:RGB")
        time.sleep(settle)

    def key(self, *names: str) -> None:
        """tmux key names: Enter, Tab, BSpace, Escape, Up, C-p, M-x, F5."""
        _tmux("send-keys", "-t", SESSION, *names)

    def ctrl_shift(self, letter: str) -> None:
        """Ctrl+Shift+<letter>, sent as CSI-u because tmux cannot spell it."""
        _tmux("send-keys", "-t", SESSION, "-l", f"\x1b[{ord(letter.lower())};6u")

    def type(self, text: str, per_char_ms: int = 90, hold_ms: int = 900) -> None:
        """Type `text` a character at a time, one frame per character."""
        for i, c in enumerate(text):
            _tmux("send-keys", "-t", SESSION, "-l", c)
            self.snap(per_char_ms if i < len(text) - 1 else hold_ms)

    def snap(self, hold_ms: int) -> None:
        """Capture the screen as the next frame, held for `hold_ms`."""
        time.sleep(0.35)
        text = _tmux("capture-pane", "-p", "-e", "-t", SESSION)
        self.frames.append((text, hold_ms))

    def save(self, gif: str | Path, still: str | Path | None = None, still_frame: int = -1) -> None:
        """Stop croft, render the frames and write the GIF (and PNG still)."""
        subprocess.run(["tmux", "kill-session", "-t", SESSION], capture_output=True)
        work = Path(tempfile.mkdtemp(prefix="croft-demo-frames-"))
        index = []
        for i, (text, ms) in enumerate(self.frames):
            (work / f"{i:04}.ans").write_text(text)
            index.append({"file": f"{i:04}.ans", "ms": ms})
        (work / "index.json").write_text(json.dumps(index))
        _ensure_node_deps()
        fonts = _ensure_fonts()
        subprocess.run(
            ["node", str(HERE / "render.mjs"), str(work), f"{self.cols}x{self.rows}", str(fonts)],
            check=True,
        )
        _write_gif(work, index, Path(gif))
        if still is not None:
            shutil.copy(work / index[still_frame]["file"].replace(".ans", ".png"), still)
        shutil.rmtree(work)
        shutil.rmtree(self.home, ignore_errors=True)


def _write_gif(work: Path, index: list[dict], out: Path) -> None:
    from PIL import Image

    imgs, durations = [], []
    for f in index:
        im = Image.open(work / f["file"].replace(".ans", ".png")).convert("RGB")
        if imgs and im.tobytes() == imgs[-1].tobytes():
            durations[-1] += f["ms"]
            continue
        imgs.append(im)
        durations.append(f["ms"])
    # One palette for every frame keeps colours from flickering between them.
    palette = imgs[0].quantize(colors=128, method=Image.Quantize.MEDIANCUT)
    frames = [im.quantize(palette=palette, dither=Image.Dither.NONE) for im in imgs]
    frames[0].save(
        out, save_all=True, append_images=frames[1:], duration=durations,
        loop=0, optimize=True, disposal=1,
    )
    print(f"{out}: {len(frames)} frames, {sum(durations) / 1000:.1f}s, {out.stat().st_size // 1024} KiB")


def _ensure_node_deps() -> None:
    if not (HERE / "node_modules/@xterm/xterm").exists():
        subprocess.run(["npm", "install", "--no-save", "--silent"], cwd=HERE, check=True)


def _ensure_fonts() -> Path:
    fonts = CACHE / "fonts"
    if not all((fonts / f).exists() for f in FONTS):
        fonts.mkdir(parents=True, exist_ok=True)
        archive = CACHE / "nerd-font.tar.xz"
        urllib.request.urlretrieve(FONT_URL, archive)
        with tarfile.open(archive) as tar:
            for f in FONTS:
                tar.extract(f, fonts, filter="data")
        archive.unlink()
    return fonts


def _write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def _tmux(*args: str) -> str:
    return subprocess.run(["tmux", *args], check=True, capture_output=True, text=True).stdout


def _q(s: str) -> str:
    return "'" + s.replace("'", "'\\''") + "'"
