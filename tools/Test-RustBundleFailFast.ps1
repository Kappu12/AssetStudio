param(
    [string]$InputPath = "A:\Blender\Bundles\maps\maps.bundle",
    [string]$OutputPath = "A:\Blender\AssetStudio_TestOutput\failfast",
    [string]$Configuration = "Release",
    [string]$TargetFramework = "net8.0-windows",
    [int]$TimeoutSeconds = 120
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$cliDll = Join-Path $repoRoot "AssetStudio.GUI\bin\$Configuration\$TargetFramework\AssetStudio.CLI.dll"

if (-not (Test-Path -LiteralPath $cliDll)) {
    throw "CLI DLL not found. Build first: dotnet build AssetStudio.CLI\AssetStudio.CLI.csproj -c $Configuration"
}

New-Item -ItemType Directory -Force -Path $OutputPath | Out-Null

$stdout = Join-Path $OutputPath "assetstudio.stdout.log"
$stderr = Join-Path $OutputPath "assetstudio.stderr.log"
$combined = Join-Path $OutputPath "assetstudio.combined.log"
Remove-Item -LiteralPath $stdout, $stderr, $combined -Force -ErrorAction SilentlyContinue

$arguments = @(
    $cliDll,
    $InputPath,
    $OutputPath,
    "--game", "Normal",
    "--export_type", "Dump",
    "--types", "MeshRenderer"
)

$process = Start-Process -FilePath "dotnet" `
    -ArgumentList $arguments `
    -RedirectStandardOutput $stdout `
    -RedirectStandardError $stderr `
    -PassThru `
    -NoNewWindow

$start = Get-Date
$lastStdoutSize = 0L
$lastStderrSize = 0L
$errorPattern = "\[Error\]|EndOfStreamException|Unable to load object"

try {
    while (-not $process.HasExited) {
        Start-Sleep -Milliseconds 500

        foreach ($path in @($stdout, $stderr)) {
            if (-not (Test-Path -LiteralPath $path)) {
                continue
            }

            $item = Get-Item -LiteralPath $path
            $previousSize = if ($path -eq $stdout) { $lastStdoutSize } else { $lastStderrSize }
            if ($item.Length -le $previousSize) {
                continue
            }

            $stream = [System.IO.File]::Open($path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
            try {
                $stream.Seek($previousSize, [System.IO.SeekOrigin]::Begin) | Out-Null
                $reader = New-Object System.IO.StreamReader($stream)
                $newText = $reader.ReadToEnd()
            }
            finally {
                $stream.Dispose()
            }

            Add-Content -LiteralPath $combined -Value $newText
            Write-Host $newText -NoNewline

            if ($path -eq $stdout) {
                $lastStdoutSize = $item.Length
            } else {
                $lastStderrSize = $item.Length
            }

            if ($newText -match $errorPattern) {
                Stop-Process -Id $process.Id -Force
                throw "AssetStudio failed fast after matching: $errorPattern"
            }
        }

        if (((Get-Date) - $start).TotalSeconds -gt $TimeoutSeconds) {
            Stop-Process -Id $process.Id -Force
            throw "AssetStudio timed out after $TimeoutSeconds seconds"
        }
    }

    foreach ($path in @($stdout, $stderr)) {
        if (Test-Path -LiteralPath $path) {
            Get-Content -LiteralPath $path | Add-Content -LiteralPath $combined
        }
    }

    if ($process.ExitCode -ne 0) {
        throw "AssetStudio exited with code $($process.ExitCode)"
    }

    "AssetStudio completed without fail-fast errors."
}
finally {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
}
