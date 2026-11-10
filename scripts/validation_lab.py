#!/usr/bin/env python3
"""Creates the test repository for the manual validation, with its local bare remote.

    python3 scripts/validation_lab.py ~/ramify-validation

Leaves `repo` (what is opened in the app), `remote.git` and `other`, a second clone to simulate
another person. Running it again deletes the folder and starts from scratch, but only if this script created it.
"""
import shutil
import sys
from pathlib import Path

from smoke_ipc import lab

MARK = ".ramify-validation"


def main():
    base = Path(sys.argv[1]).expanduser().resolve()
    if base.exists():
        if not (base / MARK).exists():
            sys.exit(f"{base} already exists and was not created by this script")
        shutil.rmtree(base)
    base.mkdir(parents=True)
    (base / MARK).touch()
    print(lab(base))


if __name__ == "__main__":
    main()
