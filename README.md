# Teddy / TeddyBench

Tools for working with the audio files of the Toniebox: dump existing Tonie files, decode them to Ogg/MP3, or create your own custom content.

This repository is a fork of [toniebox-reverse-engineering/teddy](https://github.com/toniebox-reverse-engineering/teddy) with additional features, mainly around custom tags (cover printing, random UIDs, alternative NFC reader) and exporting content.

## Components

| Project | Description |
|---|---|
| **TeddyBench** | Windows GUI. Browse the `CONTENT` folder of a Toniebox SD card, play, export, encode and manage Tonie files, and work with NFC tags. |
| **Teddy** | Command-line tool for decoding, encoding, inspecting and renaming Tonie files. |
| **TonieAudio** | Library for reading and writing the Tonie file format. |
| Concentus, Concentus.Oggfile, ID3 | Bundled third-party libraries (Opus codec, Ogg container, ID3 tags). |

## TeddyBench

### Features

- Automatically detects a Toniebox SD card (a drive with a `CONTENT` folder) and lists all Tonies with title and cover image, using `tonies.json` from [api.revvox.de](https://api.revvox.de/tonies.json).
- If a `Content` folder exists next to `TeddyBench.exe`, it is opened on startup, which is handy for a local working copy.
- Create custom Tonie files from MP3/Ogg files, rename entries, toggle the live flag, delete files.
- Export the selected Tonies (see [Exporting audio](#exporting-audio)), or use **Copy to MicroSD** to copy the selected Tonie folders to another location such as an SD card.
- Assign a new UID or a **random UID** to a Tonie file.
- **Custom tag covers:** generate a printable PDF with round cover images for the selected Tonies (*Custom tag → Create tag cover*), and a matching backside with series name and model number (*Custom tag → Create tag backside*).
- NFC support for reading tag UIDs and dumping/emulating tags with a Proxmark3 or a PN5180-based ESP reader (see below).
- *Options* menu: enable/disable downloading `tonies.json` on startup, download it now, show the debug window.

### Exporting audio

Select one or more Tonies and use one of the buttons next to the list:

| Button | What it does |
|---|---|
| **Save as .ogg...** | Dumps the tracks as `Track_01.ogg`, `Track_02.ogg`, … |
| **Save as .mp3...** | Converts each track to MP3 with ffmpeg, one track after another, with artist/album metadata. |
| **Save as .mp3 (fast)...** | Same as above, but converts the tracks in parallel (up to one ffmpeg process per CPU core). |
| **Save as .mp3 (native)...** | Converts in-process with the built-in LAME encoder (NAudio.Lame), also in parallel. Doesn't need `ffmpeg.exe`. |
| **Update covers...** | Downloads the cover image again into folders that were already exported, without touching the audio. |

Exports run in the background with a progress dialog and can be cancelled. The folder dialog starts in an `Export` folder next to `TeddyBench.exe`. You can choose whether each Tonie gets its own subfolder. Each export folder also gets a `cover.jpg`/`cover.png`. Covers are cached in `cache/covers`, and if the download fails the list-view thumbnail is used instead. Tonies whose folder already contains MP3 files are skipped, so an interrupted batch can be restarted.

### NFC readers

Select the reader with `NfcType` in `teddyBench.cfg`:

- **`Proxmark3`** (default): a Proxmark3 running the SLIX-L enabled firmware. Supports reading UIDs, unlocking tags in privacy mode, dumping, emulating, antenna measurement and firmware flashing.
- **`Pn5180Esp`**: an ESP microcontroller with a PN5180 NFC module, connected via USB serial at 115200 baud. TeddyBench detects it by sending `v` and expecting a response containing `Pn5180Esp`; `i` returns the 16-character tag UID and `u` unlocks a tag in privacy mode (reply `ok`).

### Configuration

Settings are stored as JSON in `teddyBench.cfg` next to the executable. The file is created on first start; edit it while TeddyBench is closed.

| Setting | Default | Description |
|---|---|---|
| `NfcEnabled` | `false` | Enable the NFC reader. |
| `NfcType` | `"Proxmark3"` | NFC reader type: `"Proxmark3"` or `"Pn5180Esp"`. |
| `DebugWindow` | `false` | Show the debug log window. |
| `DownloadJson` | `true` | Download `tonies.json` on startup. Set to `false` to keep a locally modified `tonies.json` (it is still downloaded if missing). |
| `CacheImageSize` | `256` | Resolution of cached cover images in pixels. |
| `CustomTagNonPrintableMargin` | `15` | Page margin the printer cannot print on, in mm. |
| `CustomTagGapX` / `CustomTagGapY` | `5` | Horizontal / vertical gap between two covers, in mm. |
| `CustomTagInitialColumn` / `CustomTagInitialRow` | `0` | Position of the first cover on the first page, so you can reuse a partially printed sheet. |
| `CustomTagSize` | `40` | Diameter of a cover, in mm. |
| `CustomTagOutlineWidth` | `0.5` | Width of the cutting outline around each cover; `0` disables it. |
| `CustomTagCoverFilename` | `"CustomTagCover.pdf"` | File name of the generated cover PDF. |
| `CustomTagBacksideFilename` | `"CustomTagBackside.pdf"` | File name of the generated backside PDF. |

Custom names you give to Tonies are stored in `customTonies.json`.

## Teddy (command line)

```
Teddy.exe -m <mode> [options] <files...>
```

| Option | Description |
|---|---|
| `-m`, `--mode=` | Operating mode: `info`, `decode`, `encode`, `rename` |
| `-o`, `--output=` | Where to write the file(s) to |
| `-p`, `--prefix=` | encode: location of prefix files |
| `-i`, `--id=` | encode: AudioID to use (default: current time) |
| `-b`, `--bitrate=` | encode: Opus bit rate in kbps |
| `--vbr` | encode: use VBR encoding |
| `-s` | decode: export as a single `.ogg` file |
| `-y` | rename: actually rename files (otherwise dry run) |
| `-d` | rename: delete duplicates |
| `-w`, `--write=` | info: write updated JSON to a local file |
| `-j`, `--json=` | JSON file/URL with details about Tonies |
| `-f`, `--format=` | Output details as `csv`, `json` or `text` |
| `-v` | Increase debug verbosity |
| `-h`, `--help` | Show help |
| `--license` | Show licenses and disclaimer |

Examples:

```
Teddy.exe -m info 500304E0                              Show header details
Teddy.exe -m decode -o out\ 500304E0                    Dump the audio as .ogg files
Teddy.exe -m encode -o 500304E0 track1.mp3 track2.mp3   Create a Tonie file from MP3 files
Teddy.exe -m encode -o out\ my-album\                   Create a Tonie file from all MP3s in a folder
Teddy.exe -m rename <source dir> <destination dir>      Move Tonie files to readable names (dry run without -y)
```

The bit rate must be a multiple of 24 kbps, otherwise the output file is incompatible. Prefix files (`0001.mp3`, `0002.mp3`, …) in the `--prefix` location are prepended to the matching track, e.g. to announce the track number.

## Building

Requirements: Windows, Visual Studio 2022 (or MSBuild) with the .NET Framework 4.8 developer pack.

1. Open `Teddy.sln` and restore NuGet packages (`nuget restore Teddy.sln`).
2. For the ffmpeg-based MP3 exports, extract `TeddyBench/Resources/ffmpeg.zip` into `TeddyBench/Resources/` so that `ffmpeg.exe` exists there. It is only shipped as a zip because the executable exceeds GitHub's file size limit, and the extracted file is git-ignored. The native MP3 export doesn't need it: its `libmp3lame` DLLs (`TeddyBench/Native/`) are embedded into `TeddyBench.exe` by Costura.
3. Build the solution, e.g. `msbuild Teddy.sln -t:rebuild -p:Configuration=Release`.

The GitHub Actions workflow in `.github/workflows/build.yml` builds Debug and Release on every push and uploads `TeddyBench.exe` as an artifact.

## Credits

Based on the work of the [toniebox-reverse-engineering](https://github.com/toniebox-reverse-engineering) community and [Hasenpups/teddy](https://github.com/Hasenpups/teddy).
