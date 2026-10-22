#!/bin/sh
# Generates the synthetic audio inputs used for the reference files in tests/fixtures/reference.
# The generated files are committed; this script only documents how they were made.
# Requires ffmpeg with libmp3lame and libopus.
set -e
cd "$(dirname "$0")"
out=inputs
rm -rf "$out"
mkdir -p "$out/album" "$out/prefix"

ff() { ffmpeg -hide_banner -loglevel error -nostdin -y "$@"; }

# tone with a bit of noise so the encoder has something to work with
tone() { # <freq> <seconds> <rate> <channels>
    echo "sine=frequency=$1:sample_rate=$3:duration=$2[a];anoisesrc=color=pink:amplitude=0.05:seed=$1:sample_rate=$3:duration=$2[b];[a][b]amix=inputs=2:normalize=0,aformat=channel_layouts=$4"
}

# album: three tracks whose ID3 track numbers do not match the file name order
ff -filter_complex "$(tone 440 4 44100 stereo)" -c:a libmp3lame -b:a 128k -id3v2_version 3 -write_xing 0 -bitexact \
    -metadata title="Second Track" -metadata track=2 -metadata album="Test Album" "$out/album/a_intro.mp3"
ff -filter_complex "$(tone 660 6 44100 stereo)" -c:a libmp3lame -b:a 128k -id3v2_version 3 -write_xing 0 -bitexact \
    -metadata title="First Track" -metadata track=1 -metadata album="Test Album" "$out/album/b_song.mp3"
ff -filter_complex "$(tone 330 3 44100 stereo)" -c:a libmp3lame -b:a 128k -id3v2_version 3 -write_xing 0 -bitexact \
    -metadata title="Third Track" -metadata track=3 -metadata album="Test Album" "$out/album/c_end.mp3"

# mono 22.05 kHz: needs resampling and upmixing
ff -filter_complex "$(tone 523 5 22050 mono)" -c:a libmp3lame -b:a 64k -id3v2_version 3 -write_xing 0 -bitexact "$out/mono22k.mp3"

# tone - digital silence - tone: silent chunks are skipped by the encoder
ff -filter_complex "sine=frequency=880:sample_rate=48000:duration=1,aformat=channel_layouts=stereo[a];anullsrc=channel_layout=stereo:sample_rate=48000:duration=2[s];sine=frequency=880:sample_rate=48000:duration=1,aformat=channel_layouts=stereo[b];[a][s][b]concat=n=3:v=0:a=1" \
    -c:a libmp3lame -b:a 128k -id3v2_version 3 -write_xing 0 -bitexact "$out/silence.mp3"

# Ogg Opus input
ff -filter_complex "$(tone 392 4 48000 stereo)" -c:a libopus -b:a 96k -bitexact "$out/opus_input.ogg"

# M4A (AAC) and Ogg Vorbis inputs; not used for the reference files, Teddy.exe could not read them
ff -filter_complex "$(tone 494 2 44100 stereo)" -c:a aac -b:a 96k -bitexact -metadata title="AAC Track" -metadata album="Test Album" "$out/aac_input.m4a"
ff -filter_complex "$(tone 587 2 44100 stereo)" -c:a libvorbis -q:a 2 -bitexact "$out/vorbis_input.ogg"

# prefix files, prepended to each track when encoding with -p
ff -filter_complex "sine=frequency=1000:sample_rate=44100:duration=0.5,aformat=channel_layouts=stereo" -c:a libmp3lame -b:a 128k -write_xing 0 -bitexact "$out/prefix/0001.mp3"
ff -filter_complex "sine=frequency=1200:sample_rate=44100:duration=0.5,aformat=channel_layouts=stereo" -c:a libmp3lame -b:a 128k -write_xing 0 -bitexact "$out/prefix/0002.mp3"
ff -filter_complex "sine=frequency=1400:sample_rate=44100:duration=0.5,aformat=channel_layouts=stereo" -c:a libmp3lame -b:a 128k -write_xing 0 -bitexact "$out/prefix/0003.mp3"

ls -lR "$out"
