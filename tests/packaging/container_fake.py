"""A small model of a qualification container's filesystem (F-M16).

The qualifier fakes used to answer ``ok()`` to any command they did not
recognise, so path checks, removal checks and preservation checks passed
whatever the package did. The fakes now keep the files a real container would
have and answer the qualifiers' shell checks from them; anything this model
does not understand fails like an unknown command.
"""

from __future__ import annotations

import re

from qualify_candidate import CommandResult

PATH_TEST_RE = re.compile(
    r'^test (-f|-x|! -e) (\S+) \|\| \{ echo "[a-z_]+=\2"; exit 1; \}$'
)


def ok(stdout: bytes = b"") -> CommandResult:
    return CommandResult(0, stdout, b"")


def unmodelled(argv: tuple[str, ...]) -> CommandResult:
    """A command the fake does not model fails, as an unknown command does in
    a real container, so no step can pass by reaching a catch-all."""
    return CommandResult(127, b"", f"unmodelled: {argv[0]}".encode())


class ContainerFs:
    def __init__(self) -> None:
        # path -> mode
        self.files: dict[str, int] = {}
        self.directories: set[str] = {"/"}

    def exists(self, path: str) -> bool:
        return path in self.files or path in self.directories

    def shell(self, script: str) -> CommandResult:
        """Run one of the qualifiers' fixed ``sh -c`` scripts: ``set``,
        ``mkdir -p``, ``chmod 700`` and ``test … || { echo …; exit 1; }``."""
        for line in script.splitlines():
            if line in ("set -eu", "set -u"):
                continue
            if line.startswith("mkdir -p "):
                self.directories.update(line[len("mkdir -p ") :].split())
                continue
            if line.startswith("chmod 700 "):
                if not all(path in self.directories for path in line.split()[2:]):
                    return CommandResult(1, b"", b"chmod: no such directory")
                continue
            match = PATH_TEST_RE.match(line)
            if match is None:
                return unmodelled(("sh", line))
            test, path = match.groups()
            if test == "-f":
                passed = path in self.files
            elif test == "-x":
                passed = self.files.get(path, 0) & 0o111 != 0
            else:
                passed = not self.exists(path)
            if not passed:
                message = line.split('echo "')[1].split('"')[0]
                return CommandResult(1, f"{message}\n".encode(), b"")
        return ok()
