"""Private audio server: tests never play through the user's desktop speakers."""
import contextlib
import os
import pathlib
import struct
import subprocess
import tempfile
import time


@contextlib.contextmanager
def audio_server():
    with tempfile.TemporaryDirectory(prefix='punto-audio-') as directory:
        socket = pathlib.Path(directory) / 'native'
        recording = pathlib.Path(directory) / 'output.pcm'
        previous = os.environ.get('PULSE_SERVER')
        os.environ['PULSE_SERVER'] = 'unix:' + str(socket)
        environment = {**os.environ, 'HOME': directory, 'XDG_CONFIG_HOME': directory, 'XDG_RUNTIME_DIR': directory}
        with open(pathlib.Path(directory) / 'server.log', 'w+') as log:
            server = subprocess.Popen([
                'pulseaudio', '-n', '--daemonize=no', '--use-pid-file=no',
                '--exit-idle-time=-1', '--disable-shm',
                '-L', 'module-null-sink sink_name=punto_test rate=44100 channels=1',
                '-L', 'module-native-protocol-unix socket=' + str(socket) + ' auth-anonymous=1',
            ], stdout=log, stderr=log, env=environment)
            capture = None
            try:
                deadline = time.monotonic() + 5
                while not socket.exists():
                    if server.poll() is not None or time.monotonic() > deadline:
                        log.seek(0)
                        raise AssertionError(log.read())
                    time.sleep(0.02)
                with recording.open('wb') as output:
                    capture = subprocess.Popen([
                        'parec', '--raw', '--format=s16le', '--rate=44100', '--channels=1',
                        '--device=punto_test.monitor', '--latency-msec=20',
                    ], stdout=output, env=environment)
                    time.sleep(0.15)
                    def check(minimum=1):
                        deadline = time.monotonic() + 4
                        while True:
                            pcm = recording.read_bytes()
                            samples = struct.unpack('<' + 'h' * (len(pcm) // 2), pcm[:len(pcm) // 2 * 2])
                            audible = sum(abs(sample) > 1000 for sample in samples)
                            if audible > minimum * 2000:
                                return
                            assert time.monotonic() < deadline, f'Insufficient correction audio: {audible} audible samples, expected > {minimum * 2000}'
                            time.sleep(0.05)
                    yield check
            finally:
                for process in (capture, server):
                    if process is not None:
                        process.terminate()
                        process.wait(timeout=5)
                if previous is None:
                    os.environ.pop('PULSE_SERVER', None)
                else:
                    os.environ['PULSE_SERVER'] = previous
