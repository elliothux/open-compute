#!/usr/bin/env python3
"""Record bounded health state from the dedicated qualification Engine."""
import argparse
import datetime
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import socket
import stat
import time


def integer(value, lower, upper):
    return value if type(value) is int and lower <= value <= upper else None


def health_record(value):
    state = value.get('State', {})
    health = state.get('Health', {}) if isinstance(state, dict) else None
    if not isinstance(health, dict):
        return {'status': 'unknown', 'checks': []}
    status = health.get('Status')
    result = {'status': status if status in ('starting', 'healthy', 'unhealthy') else 'unknown',
              'running': state.get('Running') is True,
              'failingStreak': integer(health.get('FailingStreak'), 0, (1 << 63) - 1), 'checks': []}
    logs = health.get('Log', [])
    if not isinstance(logs, list):
        return result
    for row in logs[-4:]:
        if not isinstance(row, dict):
            continue
        output = row.get('Output', '')
        if not isinstance(output, str):
            output = ''
        raw = output.encode()
        elapsed = None
        try:
            start = datetime.datetime.fromisoformat(row['Start'].replace('Z', '+00:00'))
            end = datetime.datetime.fromisoformat(row['End'].replace('Z', '+00:00'))
            duration = (end - start).total_seconds()
            if 0 <= duration <= 180:
                elapsed = round(duration * 1000)
        except (KeyError, TypeError, ValueError, AttributeError):
            pass
        result['checks'].append({'exitCode': integer(row.get('ExitCode'), -(1 << 31), (1 << 31) - 1),
            'durationMs': elapsed, 'outputBytes': len(raw), 'outputSha256': hashlib.sha256(raw).hexdigest(),
            'knownErrors': [text for text in ('no space left on device', 'disk quota exceeded',
                'permission denied', 'context deadline exceeded', 'Health check exceeded timeout') if text in output]})
    return result


def request(path, endpoint):
    before = path.lstat()
    connection = http.client.HTTPConnection('localhost', timeout=2)
    channel = socket.socket(socket.AF_UNIX)
    channel.settimeout(2)
    connection.sock = channel
    try:
        channel.connect(str(path))
        connection.request('GET', endpoint)
        response = connection.getresponse()
        data = response.read((2 << 20) + 1)
        after = path.lstat()
        if len(data) > 2 << 20 or (before.st_dev, before.st_ino, before.st_uid, before.st_mode) != (
                after.st_dev, after.st_ino, after.st_uid, after.st_mode):
            raise ValueError('ENGINE_RESPONSE_OR_IDENTITY_CHANGED')
        return response.status, json.loads(data) if data else None
    finally:
        connection.close()


def pending_exec_record(path, container, value):
    identifiers = value.get('ExecIDs')
    result = {'listed': len(identifiers) if isinstance(identifiers, list) else None}
    if not isinstance(identifiers, list):
        return result
    identifier = next((item for item in identifiers[:4] if isinstance(item, str)
                       and re.fullmatch('[0-9a-f]{64}', item)), None)
    if identifier is None:
        return result
    try:
        code, execution = request(path, '/exec/' + identifier + '/json')
        result['statusCode'] = integer(code, 100, 599)
        if code == 200:
            if not isinstance(execution, dict) or execution.get('ID') != identifier or (
                    execution.get('ContainerID') != container):
                raise ValueError('QUALIFICATION_EXEC_IDENTITY_REQUIRED')
            pid = integer(execution.get('Pid'), 0, (1 << 31) - 1)
            running = execution.get('Running')
            result.update(running=running if type(running) is bool else None,
                          processAssigned=None if pid is None else pid > 0,
                          exitCode=integer(execution.get('ExitCode'), -(1 << 31), (1 << 31) - 1))
    except Exception as error:
        result['failureClass'] = type(error).__name__
    return result


def observe(path, image):
    metadata = path.lstat()
    if not path.is_absolute() or path.resolve() != path or not stat.S_ISSOCK(metadata.st_mode) or (
            metadata.st_uid not in (0, os.getuid()) or metadata.st_mode & 0o002):
        raise ValueError('DEDICATED_ENGINE_SOCKET_REQUIRED')
    seen = False
    previous = None
    started = time.monotonic()
    deadline = started + 180
    while time.monotonic() < deadline:
        code, rows = request(path, '/containers/json?all=true')
        if code != 200 or not isinstance(rows, list):
            raise ValueError('ENGINE_INVENTORY_REQUIRED')
        found = False
        for row in rows:
            if not isinstance(row, dict) or row.get('ImageID') != image:
                continue
            identifier = row.get('Id')
            if not isinstance(identifier, str) or not re.fullmatch('[0-9a-f]{64}', identifier):
                raise ValueError('ENGINE_CONTAINER_ID_REQUIRED')
            code, value = request(path, '/containers/' + identifier + '/json')
            if code == 404:
                continue
            if code != 200 or not isinstance(value, dict) or value.get('Image') != image:
                raise ValueError('QUALIFICATION_CONTAINER_REQUIRED')
            seen = found = True
            record = health_record(value)
            if record['status'] in ('starting', 'unhealthy'):
                # Exec inspect may wait on startup; retain health even when the bounded read fails.
                record['pendingExec'] = pending_exec_record(path, identifier, value)
            if record != previous:
                print(json.dumps({'elapsedMs': round((time.monotonic() - started) * 1000),
                                  'role': 'qualification', **record}), flush=True)
                previous = record
        if seen and not found:
            return
        time.sleep(.25)
    raise ValueError('HEALTH_OBSERVATION_DEADLINE')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine-socket', type=Path, required=True)
    parser.add_argument('--image', required=True)
    args = parser.parse_args()
    try:
        if not re.fullmatch('sha256:[0-9a-f]{64}', args.image):
            raise ValueError('IMAGE_DIGEST_REQUIRED')
        observe(args.engine_socket, args.image)
    except Exception as error:
        print(json.dumps({'observationFailureClass': type(error).__name__}), flush=True)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
