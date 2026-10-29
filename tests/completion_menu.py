"""Real ZLE + daemon regression tests, isolated from user history/config."""
import os
import fcntl
import struct
import termios
from pathlib import Path
import pty
import select
import signal
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target/debug/zghost"


class CompletionMenu(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="zghost-menu-")
        self.path = Path(self.temp.name)
        self.state = self.path / "state"
        self.display = self.path / "display"
        self.log = bytearray()
        (self.path / "history").touch()
        (self.path / "native file").touch()
        (self.path / "native-dir").mkdir()
        (self.path / "native-dir" / "inner file").touch()
        env = dict(os.environ, ZDOTDIR=str(self.path), ZGHOST_SOCKET=str(self.path / "sock"),
                   XDG_CONFIG_HOME=str(self.path / "config"), XDG_DATA_HOME=str(self.path / "data"))
        self.daemon = subprocess.Popen([str(BIN), "daemon", "--foreground", "--socket", env["ZGHOST_SOCKET"],
                                        "--histfile", str(self.path / "history")], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(self.cleanup)
        (self.path / ".zshrc").write_text(f"""
PROMPT='READY> '
HISTSIZE=100
SAVEHIST=0
HISTFILE=$ZDOTDIR/history
KEYTIMEOUT=1
bindkey -e
autoload -Uz compinit
compinit -D
_audit_complete() {{ compadd alpha alpine }}
compdef _audit_complete auditcustom
_audit_files() {{ _files }}
compdef _audit_files auditfiles
_audit_prefix() {{ compset -P 'key='; compadd value-one value-two }}
compdef _audit_prefix auditprefix
_audit_attached() {{ compadd -P 'key=' value-one value-two }}
compdef _audit_attached auditattached
_audit_array() {{ local -a matches=(alpha alpine); compadd -a matches }}
compdef _audit_array auditarray

cd '{self.path}'
eval "$( '{BIN}' init zsh )"
_capture() {{
  print -r -- "$BUFFER" > "$ZDOTDIR/state.new"
  print -r -- "$CURSOR:$_zghost_menu:$_zghost_index:$#_zghost_candidates:$KEYMAP" >> "$ZDOTDIR/state.new"
  print -rl -- "$_zghost_candidates[@]" >> "$ZDOTDIR/state.new"
  print -rn -- "$POSTDISPLAY" > "$ZDOTDIR/display"
  mv "$ZDOTDIR/state.new" "$ZDOTDIR/state"
}}
functions[_audit_response]=$functions[_zghost-response]
_zghost-response() {{ _audit_response "$@"; _capture }}
_audit_redraw() {{ _zghost_pre_redraw; _zghost_pre_redraw; _capture }}
zle -N _audit_redraw
bindkey -M zghost-menu '^]' _audit_redraw '^F' zghost-accept
zle -N _capture
add-zle-hook-widget line-pre-redraw _capture
_capture
""")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 140, 0, 0))
            os.environ.update(env)
            os.execv("/bin/zsh", ["zsh", "-d", "-i"])
        self.wait(lambda s: len(s) > 1 and s[0] == "")

    def cleanup(self):
        if hasattr(self, "pid"):
            os.kill(self.pid, signal.SIGKILL)
            os.close(self.fd)
            os.waitpid(self.pid, 0)
        self.daemon.terminate()
        self.daemon.wait(timeout=3)
        self.temp.cleanup()

    def read(self):
        if select.select([self.fd], [], [], 0.02)[0]:
            self.log.extend(os.read(self.fd, 65536))
        return self.state.read_text().splitlines() if self.state.exists() else []

    def wait(self, predicate):
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline:
            state = self.read()
            if predicate(state):
                return state
        self.fail(f"State: {self.read()!r}\nTerminal: {self.log[-7000:]!r}")

    def send(self, keys):
        os.write(self.fd, keys.encode())

    def menu(self, text):
        self.send(text)
        self.wait(lambda s: s and s[0] == text)
        self.send("\t")
        return self.wait(lambda s: len(s) > 1 and s[1].split(":")[1] == "2")

    def test_fuzzy_accepts_without_executing_and_restores_keymap(self):
        state = self.menu("git chek")
        expected = state[2]
        self.send("\r")
        state = self.wait(lambda s: len(s) > 1 and s[0] == expected and s[1].split(":")[1] == "0")
        self.assertTrue(expected.startswith("git check"), expected)
        self.assertEqual(state[1].split(":")[-1], "main")
        self.send("x")
        self.wait(lambda s: s and s[0] == expected + "x")

    def test_navigation_cancel_and_edit(self):
        state = self.menu("gh pr ")
        self.assertGreater(int(state[1].split(":")[3]), 1)
        self.send("\x1b[B")
        self.wait(lambda s: len(s)>1 and s[1].split(":")[2] == "2")
        self.send("\x1b[A")
        self.wait(lambda s: len(s)>1 and s[1].split(":")[2] == "1")
        self.send("\x1b")
        self.wait(lambda s: len(s)>1 and s[1].split(":")[1] == "0")
        self.send("c")
        self.wait(lambda s: s and s[0] == "gh pr c")

    def test_unicode_midline_preserves_arguments(self):
        text = "echo é && gh pr crXX --draft"
        self.send(text)
        self.wait(lambda s: s and s[0] == text)
        self.send("\x1b[D" * len("XX --draft"))
        at = len("echo é && gh pr cr")
        self.wait(lambda s: len(s)>1 and int(s[1].split(":")[0]) == at)
        self.send("\x00")
        self.wait(lambda s: len(s)>1 and s[1].split(":")[1] == "2")
        self.send("\r")
        state = self.wait(lambda s: s and s[0] == "echo é && gh pr create --draft")
        self.assertEqual(int(state[1].split(":")[0]), len("echo é && gh pr create"))

    def test_native_candidates_feed_same_menu(self):
        state = self.menu("auditcustom a")
        self.assertIn("auditcustom alpha", state[2:])
        self.assertIn("auditcustom alpine", state[2:])
        self.send("\r")
        self.wait(lambda s: s and s[0] == "auditcustom alpha")

    def test_native_paths_escape_spaces(self):
        state = self.menu("auditfiles nat")
        self.assertTrue(any("native\\ file" in c for c in state[2:]), state)

    def test_native_ignored_prefix_is_preserved(self):
        state = self.menu("auditprefix key=v")
        self.assertIn("auditprefix key=value-one", state[2:])

    def test_native_directory_keeps_slash(self):
        state = self.menu("auditfiles native-d")
        self.assertIn("auditfiles native-dir/", state[2:])

    def test_native_visible_prefix_is_preserved(self):
        state = self.menu("auditattached key=v")
        self.assertIn("auditattached key=value-one", state[2:])

    def test_native_quoted_filename_is_valid(self):
        state = self.menu('auditfiles "native f')
        self.assertIn('auditfiles "native file"', state[2:])

    def test_native_nested_path_keeps_directory_prefix(self):
        state = self.menu("auditfiles native-dir/in")
        self.assertIn("auditfiles native-dir/inner\\ file", [c.rstrip() for c in state[2:]])

    def test_native_array_arguments_are_not_shadowed(self):
        state = self.menu("auditarray a")
        self.assertIn("auditarray alpha", state[2:])
        self.assertIn("auditarray alpine", state[2:])

    def test_unchanged_redraw_does_not_reprint_menu(self):
        self.menu("gh pr c")
        self.read()
        footer = "select · Enter/Tab insert".encode()
        before = self.log.count(footer)
        self.assertGreater(before, 0, self.log[-5000:])
        self.send("\x1d")
        deadline = time.monotonic() + 0.15
        while time.monotonic() < deadline:
            self.read()
        self.assertEqual(self.log.count(footer), before,
                         "unchanged redraw printed the completion footer again")
        self.assertEqual(self.display.read_text().count("select · Enter/Tab insert"), 1)
        self.send("\x1b")
        self.wait(lambda s: len(s)>1 and s[1].split(":")[1] == "0")
        self.assertEqual(self.display.read_text(), "")

    def test_accept_widget_never_inserts_menu_rows(self):
        state = self.menu("gh pr c")
        selected = state[2]
        self.send("\x06")
        self.wait(lambda s: len(s)>1 and s[0] == selected and s[1].split(":")[1] == "0")

    def test_typing_dismisses_menu(self):
        self.menu("gh pr c")
        self.send("r")
        self.wait(lambda s: len(s)>1 and s[0] == "gh pr cr" and s[1].split(":")[1] == "0")

    def test_auto_menu_does_not_fetch_native(self):
        self.send("ZGHOST_MENU_AUTO=1\r")
        self.wait(lambda s: s and s[0] == "")
        self.send("gh pr cr")
        self.wait(lambda s: len(s)>1 and s[0] == "gh pr cr" and s[1].split(":")[1] == "2")
        self.send("\r")
        self.wait(lambda s: s and s[0] == "gh pr create ")


if __name__ == "__main__":
    unittest.main()
