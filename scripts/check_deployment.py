#!/usr/bin/env python3
"""Render Aria/Gail/Prometheus integration with synthetic settings; never deploy."""
import argparse
import hashlib
import json
from pathlib import Path

import yaml
from jinja2 import StrictUndefined
from jinja2.nativetypes import NativeEnvironment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ansible-dir', type=Path, default=Path('../../swarmhpc/swarmhpc/ansible'))
    root = parser.parse_args().ansible_dir.resolve()
    env = NativeEnvironment(undefined=StrictUndefined)
    env.filters.update({
        'to_json': json.dumps,
        'quote': json.dumps,
        'bool': bool,
        'hash': lambda value, algorithm: hashlib.new(algorithm, value.encode()).hexdigest(),
        'to_nice_yaml': lambda value, **kwargs: yaml.safe_dump(value, **kwargs),
        'combine': lambda left, right, **_: left | right,
    })
    role = root / 'roles/continuum_tenant_aria'
    values = yaml.safe_load((role / 'defaults/main.yml').read_text()) | {
        'continuum_tenant_aria_image': 'registry.example/aria:test',
        'continuum_tenant_aria_tokens_effective': [{'principal': 'test', 'role': 'evaluator', 'token': 'x' * 48}],
        'continuum_aria_assessment_token_effective': 'y' * 48,
        'continuum_tenant_aria_webhook_secret': '',
    }
    for database, replicas in [('sqlite:///data/aria.db?mode=rwc', 1), ('postgres://test@postgres/aria', 3)]:
        context = values | {'continuum_tenant_aria_database_url': database, 'continuum_tenant_aria_replicas': replicas}
        rendered = env.from_string((role / 'templates/aria-stack.yaml.j2').read_text()).render(**context)
        documents = list(yaml.safe_load_all(rendered))
        deployment = next(item for item in documents if item['kind'] == 'Deployment')
        assert deployment['spec']['replicas'] == replicas
        assert deployment['spec']['template']['spec']['automountServiceAccountToken'] is False
        assert any(item['kind'] == 'PersistentVolumeClaim' for item in documents) == database.startswith('sqlite:')
        assert any(item['kind'] == 'PodDisruptionBudget' for item in documents) == (replicas > 1)
        secret = next(item for item in documents if item['kind'] == 'Secret')['stringData']
        assert json.loads(secret['ARIA_TOKENS'])[0]['role'] == 'evaluator'
        assert secret['ARIA_DATABASE_URL'] == database
    tasks = yaml.safe_load((root / 'roles/continuum_tenant_gail/tasks/main.yml').read_text())[0]['block']
    task = next(item for item in tasks if item.get('name') == 'Configure the independent Aria governance protocol')
    defaults = yaml.safe_load((root / 'roles/continuum_tenant_gail/defaults/main.yml').read_text())
    context = defaults | {'continuum_tenant_gail_config_effective': {'security': {'preserved': True}},
                          'continuum_tenant_gail_governance_mode': 'enforce',
                          'continuum_aria_evaluation_token_effective': 'x' * 48,
                          'continuum_aria_assessment_token_effective': 'y' * 48}
    configuration = env.from_string(task['ansible.builtin.set_fact']['continuum_tenant_gail_config_effective']).render(**context)
    assert configuration['security']['preserved']
    assert configuration['governance']['mode'] == 'enforce'
    assert configuration['governance']['evaluation_token'] != configuration['governance']['assessment_token']
    assert task['no_log'] is True
    play = yaml.safe_load((root / 'continuum_tenant_prometheus_site.yml').read_text())[0]
    context = play['vars'] | {'prometheus_aria_enabled': True, 'continuum_aria_viewer_token_effective': 'viewer-test-secret',
                              'prometheus_gail_targets': [], 'prometheus_tracey_targets': [],
                              'prometheus_refiner_targets': [], 'prometheus_conductor_targets': []}
    rendered = env.from_string((root / 'roles/continuum_tenant_k8s_app/templates/prometheus-stack.yaml.j2').read_text()).render(**context)
    documents = list(yaml.safe_load_all(rendered))
    configmap = next(item for item in documents if item['kind'] == 'ConfigMap')
    assert 'viewer-test-secret' not in json.dumps(configmap)
    scrape = yaml.safe_load(configmap['data']['prometheus.yml'])['scrape_configs'][0]
    assert scrape['authorization']['credentials_file'] == '/etc/prometheus/aria/token'
    print('PASS: SQLite and PostgreSQL manifests, Gail credential isolation, secret-backed Prometheus scraping')


if __name__ == '__main__':
    main()
