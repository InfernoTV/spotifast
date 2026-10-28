These FLAC files contain two seconds of generated silence with test tags,
at 44.1 and 48 kHz in stereo and mono. The scanner tests check that every
format supported by the pinned playback decoder is indexed. They contain
no recorded music. Generated with FFmpeg anullsrc, -t 2 -c:a flac.

`stereo-48000.opus` is an original two-second 440 Hz sine wave, encoded with
FFmpeg/libopus at 48 kbit/s, stereo 48 kHz, tagged with title `Opus Test`,
artist `Test Artist` and album `Local Fixtures`. It checks decoder-gated indexing.
