#!/bin/sh
# Shared installation step for every MagicMtn-managed sandbox image.
set -eu
python3 -m venv /opt/magicmtn/venv
/opt/magicmtn/venv/bin/pip install --no-cache-dir -r /opt/magicmtn/runtime/requirements.txt
/opt/magicmtn/venv/bin/python3 -c 'import sys; sys.path.insert(0, "/opt/magicmtn/runtime"); import agent; import websockets.sync.client; from harnesses.pi import PiHarness'
command -v pi >/dev/null
chmod 0555 /opt/magicmtn/runtime/agent.py
rm -f /usr/local/bin/magicmtn-sandbox-agent
cat > /usr/local/bin/magicmtn-sandbox-agent <<'WRAPPER'
#!/bin/sh
exec /opt/magicmtn/venv/bin/python3 /opt/magicmtn/runtime/agent.py "$@"
WRAPPER
chmod 0555 /usr/local/bin/magicmtn-sandbox-agent

chmod 0555 /opt/magicmtn/runtime/github.py
ln -s /opt/magicmtn/runtime/github.py /usr/local/bin/magicmtn-git-credential
chmod 0555 /opt/magicmtn/runtime/gh.py
ln -s /opt/magicmtn/runtime/gh.py /usr/local/bin/gh
