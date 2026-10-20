# Produces the reference files in tests/fixtures/reference with the C# Teddy.exe.
# Runs on Windows (see .github/workflows/rust-reference.yml); the output is committed.
#   generate_reference.ps1 -Teddy <path to Teddy.exe>
param([Parameter(Mandatory = $true)][string]$Teddy)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$Teddy = (Resolve-Path $Teddy).Path

if (Test-Path reference) { Remove-Item -Recurse -Force reference }
New-Item -ItemType Directory reference | Out-Null

# no tonies.json lookups, so the output does not depend on the online database
$json = @("-j", "none.json")

function Run([string]$log, [string[]]$arguments) {
    $output = & $Teddy @arguments 2>&1 | Out-String
    # .NET writes CRLF, keep LF so the files compare the same on every platform
    Set-Content -NoNewline -Encoding utf8 -Path $log -Value ($output -replace "`r`n", "`n")
}

# name, uid directory, audio id, encode arguments, sources
$cases = @(
    @{ Name = "album_cbr96";  Dir = "0A0B0C01"; Id = "0x5E034216"; Args = @();                         Src = @("inputs/album") },
    @{ Name = "album_vbr96";  Dir = "0A0B0C02"; Id = "0x5E034217"; Args = @("--vbr");                  Src = @("inputs/album") },
    @{ Name = "album_cbr64";  Dir = "0A0B0C03"; Id = "0x5E034218"; Args = @("-b", "64");               Src = @("inputs/album") },
    @{ Name = "album_prefix"; Dir = "0A0B0C04"; Id = "0x5E034219"; Args = @("-p", "inputs/prefix");    Src = @("inputs/album") },
    @{ Name = "mono22k";      Dir = "0A0B0C05"; Id = "0x0E03421A"; Args = @();                         Src = @("inputs/mono22k.mp3") },
    @{ Name = "silence";      Dir = "0A0B0C06"; Id = "0x5E03421B"; Args = @();                         Src = @("inputs/silence.mp3") },
    # the C# CLI passes the bit rate as the Opus decoder's sample rate for .ogg sources, so only 48 kbps works
    @{ Name = "opus_input";   Dir = "0A0B0C07"; Id = "0x5E03421C"; Args = @("-b", "48");               Src = @("inputs/opus_input.ogg") },
    @{ Name = "files_order";  Dir = "0A0B0C08"; Id = "0x5E03421D"; Args = @();                         Src = @("inputs/album/c_end.mp3", "inputs/mono22k.mp3") }
)

foreach ($c in $cases) {
    $caseDir = "reference/$($c.Name)"
    $uidDir = "$caseDir/$($c.Dir)"
    New-Item -ItemType Directory -Force $uidDir, "$caseDir/decode", "$caseDir/decode_single" | Out-Null
    $tonie = "$uidDir/500304E0"

    Run "$caseDir/encode.log" (@("-m", "encode", "-i", $c.Id, "-o", $tonie) + $json + $c.Args + $c.Src)
    if (-not (Test-Path $tonie)) { continue }

    Run "$caseDir/info.txt"  (@("-m", "info") + $json + @($tonie))
    Run "$caseDir/info.csv"  (@("-m", "info", "-f", "csv") + $json + @($tonie))
    Run "$caseDir/info.json" (@("-m", "info", "-f", "json") + $json + @($tonie))
    Run "$caseDir/decode.log" (@("-m", "decode", "-o", "$caseDir/decode") + $json + @($tonie))
    Run "$caseDir/decode_single.log" (@("-m", "decode", "-s", "-o", "$caseDir/decode_single") + $json + @($tonie))
}

# info over a whole directory, as used for a CONTENT folder
Run "reference/info_all.csv" (@("-m", "info", "-f", "csv") + $json + @("reference"))

Get-ChildItem -Recurse reference | Select-Object FullName, Length | Format-Table -AutoSize
