# Teddy – Rust port

Cross-platform port of Teddy/TeddyBench for Windows, Linux and macOS.

| Crate | Status | Description |
|---|---|---|
| `tonie-core` | done | Reading and writing Tonie files: header, Ogg page layout, decode, encode, `info` reports, `tonies.json` |
| `teddy` | done | Command-line tool with the same options and output as `Teddy.exe` |
| `teddybench` | in progress | GUI (egui), see below |

## Build

```
cd rust
cargo build --release
```

libopus and LAME are compiled from source, which needs CMake and a C compiler.
On Linux, audio playback also needs the ALSA headers (`libasound2-dev` on Debian/Ubuntu).

## Usage

```
teddy -m info <toniefile|directory>
teddy -m decode [-s] [-o <dir>] <toniefile|directory>
teddy -m encode [-o <file|dir>] [-i <audio id>] [-b <kbps>] [--vbr] [-p <prefix dir>] <folder | files...>
teddy -m rename [-y] [-d] <source dir> <dest dir>
```

`teddy --help` lists all options. Without `-j`, `tonies.json` is downloaded from api.revvox.de.

## TeddyBench

`teddybench [directory]` opens the given CONTENT directory, a `Content` folder next to the program,
or the first SD card with a CONTENT folder that shows up.

Implemented so far:

- Cover grid with names and covers from `tonies.json` and names from `customTonies.json`,
  icon size slider, sorting by UID, product number, date or type
- Details of the selected Tonie with its track list
- Playback with seeking
- New Tonies from audio files with track ordering, or adding an existing Tonie file
- Rename, delete, toggle the live flag, assign a new or random UID, show in the file manager
- Export to .ogg or .mp3 (ID3 tags and cover, converted in parallel), update covers,
  export the Tonie file, copy to another SD card; all with progress and cancel
- Drag and drop of audio files onto the window

Not yet: NFC readers, printing tag covers, the update check, diagnostics reports.

Data files (`teddyBench.cfg`, `tonies.json`, `customTonies.json`, `cache/`) are read from the program's
directory if a `teddyBench.cfg` is there (portable, like the C# version), otherwise from the user's data
directory (`~/.local/share/TeddyBench`, `~/Library/Application Support/TeddyBench`, `%APPDATA%\TeddyBench\data`).
The settings file is compatible with the C# version.

The live flag is the FAT hidden attribute of the Tonie file. On Linux it can only be changed on the
SD card itself (vfat), not on a copy on another file system.

## Tests

`cargo test` compares the port with output of the original C# `Teddy.exe`, stored in
`tests/fixtures/reference`. Those files are generated on Windows by the
*Rust port reference files* workflow from the synthetic inputs in `tests/fixtures/inputs`
(see `make_inputs.sh` and `generate_reference.ps1`).

- `info` (text, CSV, JSON) and `decode` output match byte for byte.
- The Ogg page layout matches byte for byte: the reference's Opus packets are replayed through the
  port's page writer. The Opus packets themselves differ, because libopus is used instead of Concentus.
- Files encoded by the port are compared structurally: header, chapters, segments per page, duration.

## Differences from Teddy.exe

Intentional fixes:

- **Track order:** folders are sorted by the ID3 track number. Teddy.exe could not read track numbers
  with a NUL terminator (as written by ffmpeg and many taggers) and silently fell back to file name order.
- **Ogg input:** `.ogg` files are always decoded at 48 kHz. Teddy.exe used the bit rate as decoder
  sample rate, so encoding Ogg files only worked with `-b 48`.
- **End of each file:** Teddy.exe encoded up to 60 ms of stale audio after every source file
  (and prefix file). Files encoded by the port are that much shorter.
- **`info -f json`** writes valid JSON (no separator before the first entry).
- **Exit code** is non-zero when something failed; a failed encode leaves no partial file behind.
- More input formats: MP3, Ogg Opus, FLAC and WAV.

Other differences:

- Resampling (rubato) and MP3 decoding (symphonia) differ from Windows Media Foundation/ACM, so the
  encoded audio is not bit-identical.
- The progress dots of `encode` depend on the decoder and differ slightly.
- `--license` lists the Rust dependencies instead of the .NET ones.
