#!/usr/bin/env python3
"""Actual PCM output, missing server and stalled handshake; no playback helpers."""
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
from sound_fixture import audio_server

binary = str(pathlib.Path(sys.argv[1]).resolve())
with audio_server() as recorded:
    subprocess.run([binary, '--test-sound'], env={**os.environ, 'PATH': '/nonexistent', 'PULSE_COOKIE': '/nonexistent'}, check=True, timeout=8)
    recorded()
with tempfile.TemporaryDirectory(prefix='punto-silent-') as directory:
    path = directory + '/socket'
    env = {**os.environ, 'PULSE_SERVER': 'unix:' + path}
    assert subprocess.run([binary, '--test-sound'], env=env, capture_output=True, timeout=8).returncode == 1
    with socket.socket(socket.AF_UNIX) as server:
        server.bind(path)
        server.listen()
        assert subprocess.run([binary, '--test-sound'], env=env, capture_output=True, timeout=8).returncode == 1
print('Built-in audio: recorded PCM, no external player, missing/stalled server OK')
