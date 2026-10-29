# Feature migration checklist

Reference: `94318fa` (managed/C++ tree). This is a development branch, not a
released replacement. A checked implementation is distinct from verification
against real game files. Update this table with evidence as work lands.

| Area | Existing behavior to preserve | Rust verification status |
| --- | --- | --- |
| Load | Files, folders, dependencies, split files, UnityFS/Web/Raw/WebData, compressed wrappers | In progress |
| Format versions | Serialized assets, type trees, stripped layouts, Unity 6000 texture/renderer changes | In progress |
| Catalog | Class, name, container, source, path ID, size, regex selection, sorting | In progress |
| Text/data | TextAsset, raw bytes, type-tree dump/JSON, MonoBehaviour JSON | In progress |
| Images | Texture2D, Sprite/Atlas, compression formats, PNG/JPEG/BMP/TGA | In progress |
| Audio/video/font | AudioClip, VideoClip, MovieTexture, Font; original payload and supported conversions | In progress |
| Mesh | Decoded vertices/indices, OBJ, preview | In progress |
| Scene | GameObject/Transform hierarchy; selected/merged model export | In progress |
| FBX | Materials/textures, skins, blend shapes, animation/controller/avatar integration | In progress |
| Lists/maps | Asset list JSON/XML, grouping by type/container/source, collision handling | In progress |
| Game variants | GameManager profiles and ImportHelper/Crypto/container variants | In progress; see game-compatibility.md |
| Recovery | asset_index container recovery, CAB maps/cache workflows | In progress |
| Managed schemas | Dummy assembly schema recovery and missing MonoBehaviour type trees | In progress |
| Desktop | Native file/folder loading, drag/drop, preview, hierarchy, filtering, export settings, diagnostics | In progress |
| CLI | Legacy positional interface, aliases, filtering, extract/map/export operations, exit codes | In progress |
| Delivery | Windows GUI+CLI ZIP after tag build/test; no Actions artifacts or caches | In progress |

## Review evidence

Sweep 1: pending implementation.
Sweep 2: pending integration and fixes.
Real game-file acceptance: no user fixture corpus has been supplied.
