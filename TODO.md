# TODO

## NFC readers

Missing compared to the former C# TeddyBench:

- Flashing the Proxmark3 firmware and bootloader (`fullimage.elf` / `bootrom.elf`), including the
  offer to flash when a device starts in bootloader mode. Currently the device is only reset and the
  log points to the Proxmark3 tools.
- A beep when a tag on the reader fills in the UID (the C# UID dialog played a system sound).

Dropped on purpose: "Report detected NFC tag" and "Report Proxmark3 antenna measurement" (they were
sent to `api.revvox.de/diag`).

Not tested yet:

- Proxmark3 with real hardware: detection, unlocking tags in privacy mode, reading and emulating tags,
  antenna measurement, console mode. Only covered by unit tests against a fake port.
- The "Tag content" and "Proxmark3 antenna" dialogs (never seen on screen).
- Finding the USB serial ports on Windows and macOS (only built in CI).
- Current stock Proxmark3 firmware: the commands use the old frame format like the C# version; newer
  firmware may expect the ISO 15693 command in the new format (see issues #63, #85).

Known rough edges:

- Turning the reader off and on again quickly can delay finding it by a minute: the old reader closes
  the port in the background, and a port that is still busy is skipped for 60 seconds.
- Cancelling an emulation only stops TeddyBench from waiting; the Proxmark3 keeps emulating until its
  button is pressed (same as C#).
- No automated test of the background reader thread and the UI reacting to its events.
- A tag put on the reader before the SD card is loaded does not select its Tonie afterwards (same as C#).
- Connecting a Proxmark3 takes a few seconds because the antenna is measured right away (same as C#).

## TRASHCAN

- Not tested with a card from a real Toniebox yet. The layout (`TRASHCAN/<folder>/<name>.043` next to
  `CONTENT`) is taken from [upstream PR #92](https://github.com/toniebox-reverse-engineering/teddy/pull/92).
- The trash file names don't tell the tag UID, so restoring always asks for it. PR #92 remembers the UID of
  Tonies it created in its own, incompatible `customTonies.json` format.
