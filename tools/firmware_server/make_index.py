#!/usr/bin/env python3
"""Write the firmware server's index.json for the camera images in a directory.

Usage: make_index.py <dir>

Each `IPC_<version>.img` becomes an entry with its sha256. Serve <dir> as-is (a bucket, or
`python3 -m http.server -d <dir>` for testing) and point FIRMWARE_SERVER_URL at it.
"""

import hashlib
import json
import sys
from pathlib import Path

directory = Path(sys.argv[1])
firmwares = []
for image in sorted(directory.glob("IPC_*.img")):
    firmwares.append(
        {
            # Matches the `version` the camera reports in getSysConfig.
            "version": image.stem.removeprefix("IPC_"),
            "file": image.name,
            "sha256": hashlib.sha256(image.read_bytes()).hexdigest(),
        }
    )

(directory / "index.json").write_text(json.dumps({"firmwares": firmwares}, indent=2) + "\n")
print(json.dumps(firmwares, indent=2))
