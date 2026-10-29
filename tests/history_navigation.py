"""Exercise real ZLE arrow keys in an isolated pseudo-terminal (stdlib only)."""
import os
from pathlib import Path
import pty
import select
import signal
import tempfile
import time
import unittest

PLUGIN = Path(__file__).resolve().parents[1] / "shell/zghost.zsh"


class HistoryNavigation(unittest.TestCase):
    def check_navigation(self, mode, prefix):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory, "state")
            Path(directory, ".zshrc").write_text(f"""
PROMPT='READY> '
HISTSIZE=100
SAVEHIST=0
HISTFILE=$ZDOTDIR/history
bindkey -{mode}
autoload -Uz up-line-or-beginning-search down-line-or-beginning-search
zle -N up-line-or-beginning-search
zle -N down-line-or-beginning-search
bindkey '{prefix}A' up-line-or-beginning-search
bindkey '{prefix}B' down-line-or-beginning-search
source '{PLUGIN}'
_zghost_send() {{ return 0 }}
_zghost_request() {{ _zghost_candidates=('echo predicted'); _zghost_index=1 }}
print -s 'echo oldest'
print -s 'echo middle'
print -s 'echo newest'
_capture() {{ print -rn -- "$BUFFER" > "$ZDOTDIR/state" }}
zle -N _capture
add-zle-hook-widget line-pre-redraw _capture
_capture
""")
            pid, fd = pty.fork()
            if pid == 0:
                os.environ["ZDOTDIR"] = directory
                os.execv("/bin/zsh", ["zsh", "-d", "-i"])
            try:
                def wait_for(expected):
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        if select.select([fd], [], [], 0.02)[0]:
                            os.read(fd, 65536)
                        if state.exists() and state.read_text() == expected:
                            return
                    actual = state.read_text() if state.exists() else "<no redraw>"
                    self.fail(f"expected {expected!r}, got {actual!r}")

                wait_for("")
                for draft in ("", "draft"):
                    os.write(fd, draft.encode())
                    wait_for(draft)
                    for suffix, expected in [
                        ("A", "echo newest"),
                        ("A", "echo middle"),
                        ("A", "echo oldest"),
                        ("B", "echo middle"),
                        ("B", "echo newest"),
                        ("B", draft),
                    ]:
                        key = (prefix + suffix).encode().decode("unicode_escape")
                        os.write(fd, key.encode())
                        wait_for(expected)
            finally:
                os.kill(pid, signal.SIGKILL)
                os.close(fd)
                os.waitpid(pid, 0)

    def test_arrows(self):
        for mode in ("e", "v"):
            for prefix in (r"\x1b[", r"\x1bO"):
                with self.subTest(mode=mode, prefix=prefix):
                    self.check_navigation(mode, prefix)


if __name__ == "__main__":
    unittest.main()
