# Changelog

Releases are tagged `rust-v<version>`. The former C# TeddyBench (`v1.x` releases) is kept in the
`csharp-backup` branch.

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

- **NFC readers:** a Proxmark3 or an ESP with a PN5180, found automatically on the USB serial ports. A tag
  put on the reader selects its Tonie and fills in the UID when assigning a UID or creating a Tonie. With the
  Proxmark3 SLIX-L firmware, tags in privacy mode are unlocked and can be read and emulated; the antenna
  measurement works with any firmware. Not yet tested with real hardware; flashing the Proxmark3 is not
  supported.

## 0.1.3 – 2026-10-20

- Select Tonies by dragging a selection frame in the grid; Ctrl/Cmd adds to the selection.

## 0.1.2 – 2026-10-20

- Linux: a desktop entry with icon, installed with `./install-desktop-entry.sh` from the archive.
- Windows: `teddybench.exe` and `teddy.exe` have the TeddyBench icon.
- Cancelling an export removes the unfinished files.
