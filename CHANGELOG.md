# Changelog

Releases are tagged `rust-v<version>`. The former C# TeddyBench (`v1.x` releases) is kept in the
`csharp-backup` branch.

## 0.1.7 – 2026-10-22

### New

- **Pause** and **previous/next track** (⏮/⏭) while playing; the toolbar shows "Track 3/9". While a Tonie
  plays, a click on a track in its details plays that track. Play, pause and stop are icons; Space plays the
  selected Tonie, pauses and continues.
- Ogg Opus files that fit a Tonie (stereo, 20 ms packets) are copied into new Tonies **without re-encoding**,
  in TeddyBench and `teddy -m encode` (upstream issue #52). Tracks exported with *Save as .ogg* or
  `teddy -m decode` come back without any loss. `teddy --reencode` encodes them anyway.
- README: how to put your own audio on a figure (upstream issue #56).

### Fixed and improved

- **Much faster start** with many Tonies: only the header and the end of each file are read, not the whole
  file – about 70 KB instead of up to 100 MB per Tonie on the SD card. 7 real Tonies (292 MB) took 3.4 s
  before and 1 ms now.
- No message about the audio output on the console when the program exits.

### Notes

- Tracks start where the Toniebox starts them when jumping: at the first page of their chapter. That is one
  page (about 340 ms) earlier than the times `teddy -m info` shows, which counts like Teddy.exe.
- Editing tracks and changing the audio ID were tested with real Tonie files: every kept track stays
  identical packet for packet. Ogg Opus files from other programs than TeddyBench are not yet tested on a real
  Toniebox.

## 0.1.6 – 2026-10-22

### New

- Tonie names, tracks and covers come from **`toniesV2.json`** of the
  [tonies-json](https://github.com/toniebox-reverse-engineering/tonies-json) repository on GitHub, which is
  updated several times a day. `tonies.json` from api.revvox.de is only used if GitHub fails.

### Fixed and improved

- **Track names are only used when their number matches the tracks of the file.** A re-release often has
  another number of tracks than the version the names were written for; its tracks were named wrongly by
  position in exports, the Edit tracks dialog and `teddy -m decode`. Fitting names are also looked for in
  other entries with the same audio or the same series and episode. The details say when the names belong
  to another version.
- Titles of Tonies without an episode no longer end in " - None".

### Notes

- The download is saved as `toniesV2.json` in the data folder. With `DownloadJson` set to `false`, the file
  changed last of `toniesV2.json` and `tonies.json` (old format, e.g. edited by hand) is used.
- `teddy -j` accepts both formats, as a file or URL.
- Track names that belong to another version are common: 3 of 7 real Tonies we checked have them.

## 0.1.5 – 2026-10-22

TeddyBench now runs on Windows, Linux and macOS as the only implementation; the C# version is no longer
developed. This release adds most of the features of the cross-platform fork
[BdN3504/teddy](https://github.com/BdN3504/teddy) and fixes several upstream issues.

### New

- **Edit tracks** of an existing Tonie (context menu): add audio files, remove and reorder tracks. The tracks
  it keeps are copied without re-encoding, so they keep their quality.
- **Deleted Tonies (TRASHCAN):** a Toniebox connected to the cloud moves deleted Tonies into a `TRASHCAN`
  folder on the card. TeddyBench lists them and restores them for a tag UID, byte for byte or as a new Tonie,
  or deletes them permanently. A Tonie already on the card for that UID can be replaced; it goes into the
  TRASHCAN instead.
- **Search** by name, UID, folder or audio ID: just start typing, Esc clears it.
- **Change audio ID** of a Tonie (context menu), without re-encoding.
- **Remove all live flags** at once (Ctrl+L, on macOS Cmd+L).
- **Keyboard:** arrow keys, Home and End select Tonies, Space plays or stops, F1 shows all shortcuts,
  F5 reads the card again.
- New Tonies are **named after the tags** of their files (album artist and album) or their folder.
- The track list of a new Tonie shows "track – artist – title", tracks can be **dragged** into another
  order, files can be added and removed; files dropped onto the window are added. Tracks are grouped by
  folder, e.g. the discs of an album.
- **M4A** (AAC, Apple Lossless) and **Ogg Vorbis** files can be encoded.
- The details show the file size.

### Fixed and improved

- `tonies.json` is downloaded from the
  [tonies-json](https://github.com/toniebox-reverse-engineering/tonies-json) repository when api.revvox.de
  fails (upstream issue #93), in TeddyBench and `teddy`. A failed download no longer replaces the local copy.
- UID fields start with `E0040350`, so only the last 8 digits of the tag have to be entered (upstream
  issue #69).
- Every new Tonie gets its own audio ID, also when several are made within a second.
- A tag on the NFC reader selects its Tonie also when the card is opened afterwards.
- An NFC reader that is turned off and on again is found again within seconds instead of a minute.

### Notes

- Edited Tonies, changed audio IDs and the TRASHCAN restore are tested with files, not yet on a real
  Toniebox. The TRASHCAN layout is taken from [upstream PR #92](https://github.com/toniebox-reverse-engineering/teddy/pull/92).
- [TODO.md](https://github.com/StefanRiese/teddy/blob/master/TODO.md) lists what is still missing or untested.

## 0.1.4 – 2026-10-22

### New

- **NFC readers:** a Proxmark3 or an ESP with a PN5180, found automatically on the USB serial ports. A tag
  put on the reader selects its Tonie and fills in the UID when assigning a UID or creating a Tonie. With the
  Proxmark3 SLIX-L firmware, tags in privacy mode are unlocked and can be read and emulated; the antenna
  measurement works with any firmware.

### Fixed and improved

- None.

### Notes

- The NFC readers are not yet tested with real hardware; flashing the Proxmark3 is not supported.

## 0.1.3 – 2026-10-20

### New

- Select Tonies by dragging a selection frame in the grid; Ctrl/Cmd adds to the selection.

### Fixed and improved

- None.

### Notes

- None.

## 0.1.2 – 2026-10-20

### New

- Linux: a desktop entry with icon, installed with `./install-desktop-entry.sh` from the archive.
- Windows: `teddybench.exe` and `teddy.exe` have the TeddyBench icon.

### Fixed and improved

- Cancelling an export removes the unfinished files.

### Notes

- None.
