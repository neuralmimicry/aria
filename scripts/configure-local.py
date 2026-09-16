#!/usr/bin/env python3
"""Create private local credentials and a Gail governance configuration overlay."""
import json
from pathlib import Path
import secrets
import shlex


root = Path(__file__).resolve().parents[1]
environment = root / 'config/local.env'
overlay = root / 'config/gail-governance.local.json'
if environment.exists() or overlay.exists():
    raise SystemExit('Local configuration already exists; refusing to replace credentials.')
tokens = {name: secrets.token_hex(24) for name in ('evaluation', 'assessment', 'operator', 'viewer')}
values = {
    'ARIA_GAIL_URL': 'http://127.0.0.1:8080',
    'ARIA_GAIL_TOKEN': tokens['assessment'],
    'ARIA_EVALUATION_TOKEN': tokens['evaluation'],
    'ARIA_OPERATOR_TOKEN': tokens['operator'],
    'ARIA_VIEWER_TOKEN': tokens['viewer'],
    'ARIA_TOKENS': json.dumps([
        {'principal': 'gail', 'role': 'evaluator', 'token': tokens['evaluation']},
        {'principal': 'local-operator', 'role': 'operator', 'token': tokens['operator']},
        {'principal': 'local-viewer', 'role': 'viewer', 'token': tokens['viewer']},
    ]),
}
for path, content in [(environment, ''.join(f'export {key}={shlex.quote(value)}\n' for key, value in values.items())),
                      (overlay, json.dumps({'governance': {
                          'mode': 'enforce', 'aria_url': 'http://127.0.0.1:8091',
                          'evaluation_token': tokens['evaluation'], 'assessment_token': tokens['assessment'],
                          'timeout_ms': 20000, 'assessment_timeout_ms': 12000,
                          'max_body_bytes': 262144, 'max_in_flight': 32, 'fail_open': False,
                      }}, indent=2) + '\n')]:
    # Create files privately from the outset; there is no permissive chmod window.
    import os
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'w') as handle:
        handle.write(content)
print('Created config/local.env and config/gail-governance.local.json with mode 0600.')

