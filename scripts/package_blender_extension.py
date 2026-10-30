"""Package the standalone Blender extension without user settings or caches."""
import argparse
from pathlib import Path
import zipfile

FILES = ("__init__.py", "blender_manifest.toml", "rust_shader.blend", "NOTICE.md", "LICENSE.txt")


def package(output, root=None):
    root = root or Path(__file__).resolve().parents[1]
    source = root / "blender_extension" / "assetstudio_bridge"
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name in FILES:
            archive.write(source / name, name)
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    print(package(parser.parse_args().output))
