#!/usr/bin/env python3
"""Native server contract fixture: real HTTP, SSE, and an engine subprocess."""
import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

parser = argparse.ArgumentParser()
parser.add_argument('--port', type=int, required=True)
parser.add_argument('--alias')
parser.add_argument('--config')
args, _ = parser.parse_known_args()
if os.environ.get('FAIL_START'):
    sys.exit(7)
if args.config:
    with open(args.config) as source:
        native = json.load(source)
    assert native['lazy_load'] is False
    assert native['idle_unload_s'] == 0
    model = native['model_name']
else:
    model = args.alias

child = subprocess.Popen([sys.executable, '-c', 'import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(600)'])
with open(os.environ['EVENTS'], 'a') as events:
    events.write(json.dumps({'model': model, 'pid': os.getpid(), 'child': child.pid}) + '\n')
started = time.monotonic()
cached_input = None

class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_):
        pass
    def send_json(self, body, status=200):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        if self.path == '/health':
            ready = time.monotonic() - started >= float(os.environ.get('READY_DELAY', '0'))
            self.send_json({'status': 'ok', 'loaded': ready})
        elif self.path == '/v1/models':
            self.send_json({'data': [{'id': model}]})
        else:
            self.send_json({}, 404)
    def do_POST(self):
        global cached_input
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path.startswith('/slots/0?action='):
            action = self.path.split('=')[1]
            directory = Path(native['slot_save_path'])
            directory.mkdir(parents=True, exist_ok=True)
            target = directory / body['filename']
            marker = Path(native['cwd']) / ('session-' + action)
            if marker.exists():
                mode = marker.read_text()
                if mode == 'fail':
                    self.send_json({'error': {'message': 'session failure'}}, 500)
                    return
                time.sleep(float(mode))
            if action == 'save':
                target.write_text(json.dumps(cached_input))
                self.send_json({'id_slot': 0, 'filename': target.name, 'n_saved': 1, 'n_written': target.stat().st_size})
            else:
                try:
                    cached_input = json.loads(target.read_text())
                except (ValueError, FileNotFoundError):
                    self.send_json({'error': {'message': 'invalid session'}}, 400)
                    return
                self.send_json({'id_slot': 0, 'filename': target.name, 'n_restored': 1, 'n_read': target.stat().st_size})
            return
        previous_input = cached_input
        cached_input = body.get('input')
        if body.get('input') == 'error':
            self.send_json({'error': {'code': 'context_length_exceeded'}}, 400)
        elif body.get('stream'):
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(b'event: response.created\ndata: {"type":"response.created"}\n\n')
            self.wfile.flush()
            time.sleep(float(body.get('delay', 0.5)))
            self.wfile.write('event: response.output_text.delta\ndata: {"delta":"日本語"}\n\nevent: response.completed\ndata: {"type":"response.completed"}\n\n'.encode())
            self.wfile.flush()
            self.close_connection = True
        else:
            self.send_json({'model': model, 'path': self.path, 'request': body, 'cached_input': previous_input})

ThreadingHTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
