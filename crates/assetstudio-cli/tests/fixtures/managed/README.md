# Managed schema acceptance fixtures

These small test assemblies, their C# source, and reference tree snapshots are
copied from the MIT-licensed `unity-typetree-gen` crate version 0.3.2:
<https://github.com/jakobhellermann/unity-typetree-gen>.
`LICENSE.txt` retains the original copyright/license notice.

They are synthetic test assemblies, including fake UnityEngine definitions;
none are proprietary game files or official Unity runtime assemblies. They
are read as PE/CLI metadata by Rust tests and are never executed. Tests require
no .NET runtime or compiler. Source files are provenance, not application code.

The upstream snapshots were generated using AssetsTools.NET and independently
check primitive field types, order and alignment for Unity 5, 2019 and 6000.
The generator's snapshots cover script fields; AssetStudio prepends the
engine-owned MonoBehaviour player header when constructing a full schema.

These fixture DLLs are not included in desktop/CLI release ZIPs.
