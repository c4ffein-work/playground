#!/usr/bin/env python3
"""Plays the scenarios of README.md on real devices, through a real drop zone.

    sudo PASS_PY=/path/to/pass.py ./run.py [--kinds py,core,...] [--keep] [scenario number...]

A device is either pass.py with gpg, git and sftp ("py"), or the phone's core through its
pass-device program ("core"). Every scenario is played on every mix of kinds given by --kinds
(default: all of one kind, then each mix). Needs root: the drop zone is ../dropzone/entrypoint.sh.
"""

import itertools
import os
import random
import shutil
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, '..', '..', '..'))  # ios-apps
ENTRYPOINT = os.path.join(HERE, '..', 'dropzone', 'entrypoint.sh')
PASS_PY = os.environ.get('PASS_PY', '')
PASS_DEVICE = os.environ.get('PASS_DEVICE', os.path.join(ROOT, 'target', 'debug', 'examples', 'pass-device'))
VOCABULARY = ('added ', 'changed ', 'moved ', 'removed ', 'readers ', 'stale ', 'conflict ')


class Failed(Exception):
    """A scenario did not go as README.md says."""


class CommandFailed(Exception):
    """A device refused to do something."""


def expect(condition, message):
    if not condition:
        raise Failed(message)


def run(command, env=None, text_in=None, cwd=None):
    result = subprocess.run(command, env=env, input=text_in, cwd=cwd, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True)
    if result.returncode != 0:
        raise CommandFailed(f"{' '.join(command[:6])}...: {result.stderr.strip() or result.stdout.strip()}")
    return result.stdout


class DropZone:
    """../dropzone/entrypoint.sh on a free port, with the SSH keys of the devices."""

    def __init__(self, work, ssh_public_keys):
        self.data = os.path.join(work, 'zone')
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            self.port = probe.getsockname()[1]
        env = dict(os.environ, DROPZONE_DATA=self.data, DROPZONE_PORT=str(self.port),
                   DROPZONE_AUTHORIZED_KEYS='\n'.join(ssh_public_keys))
        self.log = open(os.path.join(work, 'zone.log'), 'w+')
        self.process = subprocess.Popen([ENTRYPOINT], env=env, stdout=self.log, stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                socket.create_connection(('127.0.0.1', self.port), 1).close()
                break
            except OSError:
                if self.process.poll() is not None:
                    break
                time.sleep(0.05)
        else:
            raise Failed("the drop zone did not start")
        self.log.seek(0)
        marker = 'host key '
        fingerprints = [line.split(marker)[1].strip() for line in self.log.read().splitlines() if marker in line]
        if not fingerprints:
            raise Failed("the drop zone did not start: " + open(self.log.name).read())
        self.host_key = fingerprints[0]
        self.url = f'sftp://drop@127.0.0.1:{self.port}'
        self.known_hosts = os.path.join(work, 'known_hosts')
        with open(self.known_hosts, 'w') as known:
            known.write(run(['ssh-keyscan', '-p', str(self.port), '127.0.0.1']))
        self.files = os.path.join(self.data, 'jail', 'drop')

    def stop(self):
        self.process.terminate()
        self.process.wait()
        self.log.close()


class Device:
    """What a scenario asks of a device, whatever its kind."""

    kind = '?'

    def __init__(self, name, home, zone):
        self.name, self.home, self.zone = name, home, zone
        self.ssh_key = os.path.join(home, 'ssh')
        self.fingerprint = None
        self.fleet = None  # name -> Device, set once all are made

    def others(self):
        return [device for device in self.fleet.values() if device is not self]

    def fingerprints(self, names):
        return [self.fleet[name].fingerprint for name in names]

    def can(self, action, *args, **kwargs):
        """Whether an action goes through: (True, its result) or (False, the refusal)."""
        try:
            return True, getattr(self, action)(*args, **kwargs)
        except CommandFailed as refusal:
            return False, str(refusal)

    def reads(self, name):
        return self.name in self.readers(name.rsplit('/', 1)[0] + '/' if '/' in name else '/')


class PyDevice(Device):
    """pass.py, with its own gpg home, as a computer would be."""

    kind = 'py'

    def __init__(self, name, home, zone):
        super().__init__(name, home, zone)
        self.store = os.path.join(home, 'store')
        self.env = dict(os.environ, HOME=home, GNUPGHOME=os.path.join(home, 'g'), PASSWORD_STORE_DIR=self.store,
                        GIT_CONFIG_GLOBAL=os.path.join(home, 'gitconfig'), GIT_CONFIG_NOSYSTEM='1')
        for variable in ('GIT_DIR', 'GIT_WORK_TREE', 'PASSWORD_STORE_KEY', 'PASSWORD_STORE_SIGNING_KEY'):
            self.env.pop(variable, None)

    def gpg(self, *args, text_in=None):
        return run(['gpg', '--batch', '--quiet'] + list(args), env=self.env, text_in=text_in)

    def git(self, *args):
        return run(['git', '-C', self.store] + list(args), env=self.env)

    def passpy(self, *args, text_in=None):
        return run([sys.executable, PASS_PY] + list(args), env=self.env, text_in=text_in)

    def make_key(self, label):
        self.gpg('--passphrase', '', '--quick-generate-key', f'{label} <{label}@example.invalid>',
                 'ed25519', 'sign', 'never')
        listing = self.gpg('--with-colons', '--list-keys', f'{label}@example.invalid')
        fingerprint = next(line.split(':')[9] for line in listing.splitlines() if line.startswith('fpr'))
        self.gpg('--passphrase', '', '--quick-add-key', fingerprint, 'cv25519', 'encr', 'never')
        return fingerprint

    def init(self):
        os.makedirs(self.env['GNUPGHOME'], mode=0o700)
        with open(self.env['GIT_CONFIG_GLOBAL'], 'w') as config:
            config.write(f'[user]\n\tname = {self.name}\n\temail = {self.name}@example.invalid\n'
                         '[init]\n\tdefaultBranch = main\n')
        self.fingerprint = self.make_key(self.name)
        self.passpy('sync', 'setup', self.name, self.zone.url, self.fingerprint)
        self.git('config', 'pass-sync.sftp-options',
                 f'-i {self.ssh_key} -o UserKnownHostsFile={self.zone.known_hosts} -o StrictHostKeyChecking=yes '
                 '-o IdentitiesOnly=yes -o BatchMode=yes')

    def public_key(self):
        return self.gpg('--armor', '--export', self.fingerprint)

    def peer(self, other):
        self.gpg('--import', text_in=other.public_key())
        # A computer's user has said they trust the keys of their own devices.
        self.gpg('--import-ownertrust', text_in=f'{other.fingerprint}:6:\n')
        self.passpy('sync', 'peer', other.name, other.fingerprint)

    def create(self, readers):
        self.passpy('init', *self.fingerprints(readers))

    def insert(self, name, content):
        self.passpy('insert', '-m', '-f', name, text_in=content + '\n')

    def show(self, name):
        return self.passpy('show', name).rstrip('\n')

    def rm(self, name):
        self.passpy('rm', '-r', '-f', name.rstrip('/'))

    def mv(self, source, target):
        self.passpy('mv', '-f', source.rstrip('/'), target.rstrip('/'))

    def ls(self):
        files = self.git('ls-files', '-z', '--', '*.gpg').split('\0')
        return sorted(path[:-4] for path in files if path)

    def readers_file(self, directory):
        return os.path.join(self.store, directory.strip('/'), '.gpg-id')

    def readers(self, directory):
        """The readers that govern a folder, its own or inherited."""
        while not os.path.isfile(self.readers_file(directory)):
            directory = os.path.dirname(directory.strip('/'))
        names = {device.fingerprint: device.name for device in self.fleet.values()}
        with open(self.readers_file(directory)) as lines:
            return sorted(names.get(line.strip(), '?') for line in lines if line.strip())

    def set_readers(self, directory, readers):
        where = ['-p', directory.strip('/')] if directory.strip('/') else []
        if not readers:
            self.passpy('init', *where, '')
        elif self.reads(directory.strip('/') + '/x' if directory.strip('/') else 'x'):
            self.passpy('init', *where, *self.fingerprints(readers))
        else:
            # Asking to read what one cannot: there is nothing to re-encrypt, only the file to change.
            os.makedirs(os.path.dirname(self.readers_file(directory)), exist_ok=True)
            with open(self.readers_file(directory), 'w') as lines:
                lines.write(''.join(f'{fingerprint}\n' for fingerprint in self.fingerprints(readers)))
            self.git('add', self.readers_file(directory))
            self.git('commit', '--quiet', '-m', f'Set GPG id for {directory}.')

    def stale(self):
        return sorted(line[6:] for line in self.passpy('sync', 'status').splitlines() if line.startswith('stale '))

    def sync(self):
        """Publish and receive: {device: 'take' | 'nothing' | 'refused' | 'ignored' | 'none'}, and under
        '' why this device did not publish, when it did not."""
        states = {'': None}
        if self.git('rev-parse', '--verify', '--quiet', 'HEAD^{commit}') if self.has_history() else False:
            published, refusal = self.can('passpy', 'sync', 'push')
            states[''] = None if published else refusal
        result = subprocess.run([sys.executable, PASS_PY, 'sync', 'fetch'], env=self.env, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True)
        if result.returncode != 0 and 'REFUSED' not in result.stdout:
            raise CommandFailed(f"sync fetch: {result.stderr.strip()}")
        for line in result.stdout.splitlines():
            name, _, rest = line.partition(': ')
            states[name] = ('refused' if rest.startswith('REFUSED') else 'ignored' if rest.startswith('IGNORED') else
                            'take' if 'to take' in rest and 'nothing to take' not in rest else
                            'nothing' if 'nothing to take' in rest else 'none')
            states[name + ':why'] = rest
        return states

    def has_history(self):
        return subprocess.run(['git', '-C', self.store, 'rev-parse', '--verify', '--quiet', 'HEAD^{commit}'],
                              env=self.env, stdout=subprocess.DEVNULL).returncode == 0

    def pending(self, other):
        lines = self.passpy('sync', 'log', other).splitlines()
        return sorted(line for line in lines if line.startswith(VOCABULARY))

    def take(self, other, mine=(), theirs=()):
        choices = [part for name in mine for part in ('--mine', name)]
        choices += [part for name in theirs for part in ('--theirs', name)]
        self.passpy('sync', 'merge', other, *choices)

    def drop(self, other):
        self.git('reset', '--quiet', '--hard', f'refs/pass-sync/{other}')
        self.passpy('sync', 'push')

    # What only the scenarios about a device gone wrong need: publishing without pass.py's checks.

    def sftp(self, *commands):
        options = self.git('config', 'pass-sync.sftp-options').split()
        run(['sftp', '-q', '-b', '-', '-P', str(self.zone.port)] + options + ['drop@127.0.0.1'],
            env=self.env, text_in=''.join(f'{command}\n' for command in commands))

    def seal(self, bundle, recipients=None, signer=None):
        message = os.path.join(self.home, 'raw.message')
        command = ['--yes', '--trust-model', 'always', '--compress-algo', 'none', '--digest-algo', 'SHA512',
                   '--local-user', signer or self.fingerprint, '--sign', '--encrypt']
        for device in (recipients or self.fleet.values()):
            command += ['--recipient', device.fingerprint]
        self.gpg(*command, '--output', message, bundle)
        return message

    def raw_bundle(self, *references):
        bundle = os.path.join(self.home, 'raw.bundle')
        self.git('update-ref', f'refs/pass-sync/{self.name}', 'HEAD')
        self.git('bundle', 'create', '--quiet', '--version=2', bundle, *(references or [f'refs/pass-sync/{self.name}']))
        return bundle

    def raw_publish(self, message=None, name=None):
        message = message or self.seal(self.raw_bundle())
        target = f'{name or self.name}.bundle.gpg'
        self.sftp(f'put {message} {target}.part', f'rename {target}.part {target}')


class CoreDevice(Device):
    """The phone's core, through its pass-device program."""

    kind = 'core'

    def __init__(self, name, home, zone):
        super().__init__(name, home, zone)
        self.env = dict(os.environ, PASS_DEVICE_HOME=os.path.join(home, 'device'))

    def device(self, *args):
        return run([PASS_DEVICE] + list(args), env=self.env)

    def lines(self, *args):
        return [line for line in self.device(*args).splitlines() if line]

    def init(self):
        self.fingerprint = self.device('init', self.name, '--dropzone', self.zone.url, '--host-key',
                                       self.zone.host_key, '--ssh-key', self.ssh_key).strip()

    def public_key(self):
        return self.device('public-key')

    def peer(self, other):
        path = os.path.join(self.home, f'{other.name}.asc')
        with open(path, 'w') as key:
            key.write(other.public_key())
        self.device('peer', other.name, path)

    def create(self, readers):
        self.device('create', *readers)

    def insert(self, name, content):
        self.device('insert', name, content)

    def show(self, name):
        return self.device('show', name).rstrip('\n')

    def rm(self, name):
        self.device('rm', name)

    def mv(self, source, target):
        self.device('mv', source, target)

    def ls(self):
        return self.lines('ls')

    def readers(self, directory):
        """The readers that govern a folder, its own or inherited."""
        return sorted(self.lines('readers', directory))

    def set_readers(self, directory, readers):
        self.device('set-readers', directory, *readers)

    def stale(self):
        return self.lines('stale')

    def sync(self):
        states = {'': None}
        result = subprocess.run([PASS_DEVICE, 'sync'], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                text=True)
        if result.returncode != 0:
            states[''] = result.stderr.strip() or 'failed'
        for line in result.stdout.splitlines():
            name, _, rest = line.partition(': ')
            states[name] = ('refused' if rest.startswith('REFUSED') else 'ignored' if rest.startswith('IGNORED') else
                            'take' if rest.endswith('to take') and not rest.startswith('nothing') else
                            'nothing' if rest.startswith('nothing to take') else 'none')
            states[name + ':why'] = rest
        return states

    def pending(self, other):
        return sorted(line for line in self.lines('pending', other) if line.startswith(VOCABULARY))

    def take(self, other, mine=(), theirs=()):
        choices = [part for name in mine for part in ('--mine', name)]
        choices += [part for name in theirs for part in ('--theirs', name)]
        self.device('take', other, *choices)

    def drop(self, other):
        self.device('drop', other)


KINDS = {'py': PyDevice, 'core': CoreDevice}


class Fleet:
    """Devices that know each other, and their drop zone. `a`, `b`, `c` by attribute."""

    def __init__(self, kinds, work):
        self.work = tempfile.mkdtemp(dir=work)
        os.chmod(self.work, 0o755)
        names = 'abc'[:len(kinds)]
        keys = []
        for name in names:
            os.makedirs(os.path.join(self.work, name))
            run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', name, '-f', os.path.join(self.work, name, 'ssh')])
            keys.append(open(os.path.join(self.work, name, 'ssh.pub')).read().strip())
        self.zone = DropZone(self.work, keys)
        self.devices = {name: KINDS[kind](name, os.path.join(self.work, name), self.zone)
                        for name, kind in zip(names, kinds)}
        for device in self.devices.values():
            device.fleet = self.devices
            device.init()
        for device in self.devices.values():
            for other in device.others():
                device.peer(other)

    def __getattr__(self, name):
        try:
            return self.devices[name]
        except KeyError:
            raise AttributeError(name) from None

    def all(self):
        return list(self.devices.values())

    def stop(self):
        self.zone.stop()
        for device in self.devices.values():
            if isinstance(device, PyDevice):
                subprocess.run(['gpgconf', '--kill', 'all'], env=device.env, stderr=subprocess.DEVNULL)


def settle(devices, resolve=None, only=None):
    """Everybody syncs and takes what there is, until nothing moves.
    resolve: what to answer where a choice is needed, as a function (device, other, name) -> 'mine' | 'theirs'.
    only: {device name: the device names it takes from}, for those that do not take from everybody."""
    quiet = 0
    for _ in range(16):
        moved = False
        for device in devices:
            states = device.sync()
            expect(states[''] is None, f"{device.name} could not publish: {states['']}")
            for other in device.others():
                if states.get(other.name) != 'take' or (only and other.name not in only.get(device.name, [other.name])):
                    continue
                mine, theirs = [], []
                if resolve:
                    for line in device.pending(other.name):
                        if line.startswith('conflict '):
                            (mine if resolve(device, other, line[9:]) == 'mine' else theirs).append(line[9:])
                device.take(other.name, mine=mine, theirs=theirs)
                moved = True
        # One quiet round is not enough: the first to sync had not seen what the last then published.
        quiet = 0 if moved else quiet + 1
        if quiet == 2:
            return
    raise Failed("the devices never stop having something to take from each other")


def check(devices, contents, only=None):
    """All devices hold these entries, read them where they are readers and not elsewhere,
    and have nothing stale and nothing left to take."""
    for device in devices:
        expect(device.ls() == sorted(contents), f"{device.name} lists {device.ls()}, not {sorted(contents)}")
        expect(device.stale() == [], f"{device.name} has stale entries: {device.stale()}")
        for name, content in contents.items():
            readable, shown = device.can('show', name)
            if device.reads(name):
                expect(readable and shown == content, f"{device.name} reads {name} as {shown!r}, not {content!r}")
            else:
                expect(not readable, f"{device.name} is no reader of {name} and reads it: {shown!r}")
        states = device.sync()
        for other in device.others():
            if not only or other.name in only.get(device.name, [other.name]):
                expect(states.get(other.name) == 'nothing',
                       f"{device.name} still has this from {other.name}: {states.get(other.name + ':why')}")


def started(make, count=2, readers=None, entries=None):
    """A fleet whose devices all hold the same store: root readers (all by default), these entries."""
    fleet = make(count)
    names = list(fleet.devices)
    fleet.a.create(readers or names)
    for name, content in (entries or {'seed': 's'}).items():
        fleet.a.insert(name, content)
    settle(fleet.all())
    return fleet


def both_sync(fleet):
    """a and b publish what they have, and a receives b's: a now has b's changes to take."""
    fleet.a.sync()
    fleet.b.sync()
    states = fleet.a.sync()
    expect(states['b'] == 'take', f"a has nothing to take from b: {states.get('b:why')}")


def unchanged(device, action, *args, **kwargs):
    """The action is refused, and the device's entries are what they were."""
    before = device.ls()
    done, result = device.can(action, *args, **kwargs)
    expect(not done, f"{device.name} did {action} {args} {kwargs}, which it must refuse")
    expect(device.ls() == before, f"{device.name}'s entries changed although it refused: {device.ls()}")
    return result


def s01(make):
    fleet = make(2)
    fleet.a.create(['a', 'b'])
    fleet.a.insert('one', '1')
    fleet.a.sync()
    states = fleet.b.sync()
    expect(states['a'] == 'take', f"b sees nothing to adopt: {states.get('a:why')}")
    expect(fleet.b.pending('a') == ['added one', 'readers /: a b'], f"b is shown {fleet.b.pending('a')}")
    fleet.b.take('a')
    settle(fleet.all())
    check(fleet.all(), {'one': '1'})


def s02(make):
    fleet = started(make)
    fleet.a.insert('x', '1')
    settle(fleet.all())
    fleet.b.insert('y', '2')
    settle(fleet.all())
    check(fleet.all(), {'seed': 's', 'x': '1', 'y': '2'})


def s03(make):
    fleet = started(make)
    fleet.a.insert('x', '1')
    fleet.b.insert('y', '2')
    both_sync(fleet)
    expect(fleet.a.pending('b') == ['added y'], f"a is shown {fleet.a.pending('b')}")
    settle(fleet.all())
    check(fleet.all(), {'seed': 's', 'x': '1', 'y': '2'})


def two_versions(make, choice, expected):
    fleet = started(make)
    fleet.a.insert('seed', 'A')
    fleet.b.insert('seed', 'B')
    both_sync(fleet)
    expect(fleet.a.pending('b') == ['changed seed', 'conflict seed'], f"a is shown {fleet.a.pending('b')}")
    unchanged(fleet.a, 'take', 'b')
    expect(fleet.a.show('seed') == 'A', "a's entry changed although nothing was taken")
    fleet.a.take('b', **{choice: ['seed']})
    settle(fleet.all())
    check(fleet.all(), {'seed': expected})


def s04(make):
    two_versions(make, 'mine', 'A')


def s05(make):
    two_versions(make, 'theirs', 'B')


def s06(make):
    for choice, expected in (('mine', {}), ('theirs', {'seed': 'B'})):
        fleet = started(make, entries={'seed': 's', 'other': 'o'})
        fleet.a.rm('seed')
        fleet.b.insert('seed', 'B')
        both_sync(fleet)
        expect(fleet.a.pending('b') == ['changed seed', 'conflict seed'], f"a is shown {fleet.a.pending('b')}")
        unchanged(fleet.a, 'take', 'b')
        fleet.a.take('b', **{choice: ['seed']})
        settle(fleet.all())
        check(fleet.all(), dict(expected, other='o'))


def s07(make):
    fleet = started(make)
    fleet.a.mv('seed', 'moved')
    fleet.b.insert('seed', 'new')
    fleet.a.sync()
    states = fleet.b.sync()
    expect(states['a'] == 'take', "b has nothing to take from a")
    expect(fleet.b.pending('a') == ['moved seed -> moved'], f"b is shown {fleet.b.pending('a')}")
    settle(fleet.all())
    check(fleet.all(), {'moved': 'new'})


def s08(make):
    fleet = started(make, entries={'dir/one': '1', 'dir/two': '2', 'dir/three': '3'})
    fleet.a.mv('dir/', 'new/')
    fleet.b.insert('dir/four', '4')
    fleet.b.insert('dir/two', '2b')
    settle(fleet.all())
    check(fleet.all(), {'new/one': '1', 'new/two': '2b', 'new/three': '3', 'dir/four': '4'})


def s09(make):
    for choice, name, expected in (('mine', 'pa', 'pa'), ('theirs', 'pa', 'pb')):
        fleet = started(make)
        fleet.a.mv('seed', 'pa')
        fleet.b.mv('seed', 'pb')
        both_sync(fleet)
        expect(fleet.a.pending('b') == ['conflict pa', 'moved seed -> pb'], f"a is shown {fleet.a.pending('b')}")
        unchanged(fleet.a, 'take', 'b')
        fleet.a.take('b', **{choice: [name]})
        settle(fleet.all())
        check(fleet.all(), {expected: 's'})


def s10(make):
    fleet = started(make)
    fleet.a.insert('seed', 'same')
    fleet.b.insert('seed', 'same')
    both_sync(fleet)
    expect('conflict seed' not in fleet.a.pending('b'), f"a is shown {fleet.a.pending('b')}")
    settle(fleet.all())
    check(fleet.all(), {'seed': 'same'})


def s11(make):
    for choice, expected in (('mine', 'A'), ('theirs', 'B')):
        fleet = started(make)
        fleet.a.insert('new', 'A')
        fleet.b.insert('new', 'B')
        both_sync(fleet)
        expect(fleet.a.pending('b') == ['added new', 'conflict new'], f"a is shown {fleet.a.pending('b')}")
        unchanged(fleet.a, 'take', 'b')
        fleet.a.take('b', **{choice: ['new']})
        settle(fleet.all())
        check(fleet.all(), {'seed': 's', 'new': expected})


def s12(make):
    # Neither entry is lost to the other: "theirs" is b's y, and the moved one back where b has it.
    for choice, expected in (('mine', {'y': 's'}), ('theirs', {'y': 'B', 'seed': 's'})):
        fleet = started(make)
        fleet.a.mv('seed', 'y')
        fleet.b.insert('y', 'B')
        both_sync(fleet)
        expect(fleet.a.pending('b') == ['added y', 'conflict y'], f"a is shown {fleet.a.pending('b')}")
        unchanged(fleet.a, 'take', 'b')
        fleet.a.take('b', **{choice: ['y']})
        settle(fleet.all())
        check(fleet.all(), expected)


def shared_folder(make, count=2, shared=('a', 'b')):
    """The root read by a alone, shared/ by a and b."""
    fleet = make(count)
    fleet.a.create(['a'])
    fleet.a.insert('private', 'p')
    fleet.a.insert('shared/s1', '1')
    fleet.a.set_readers('shared/', list(shared))
    settle(fleet.all())
    return fleet


def s13(make):
    fleet = shared_folder(make)
    expect(fleet.b.readers('shared/') == ['a', 'b'], f"b sees the readers of shared/ as {fleet.b.readers('shared/')}")
    check(fleet.all(), {'private': 'p', 'shared/s1': '1'})


def s14(make):
    fleet = shared_folder(make)
    fleet.b.insert('written-blind', 'x')
    settle(fleet.all())
    check(fleet.all(), {'private': 'p', 'shared/s1': '1', 'written-blind': 'x'})


def s15(make):
    fleet = shared_folder(make, count=3)
    expect(not fleet.c.can('show', 'shared/s1')[0], "c reads shared/ before being given it")
    fleet.a.set_readers('shared/', ['a', 'b', 'c'])
    fleet.a.sync()
    fleet.c.sync()
    expect(fleet.c.pending('a') == ['readers shared/: a b c'] or
           fleet.c.pending('a') == ['changed shared/s1', 'readers shared/: a b c'],
           f"c is shown {fleet.c.pending('a')}")
    settle(fleet.all())
    check(fleet.all(), {'private': 'p', 'shared/s1': '1'})


def request(make):
    """b asks to read work/, which a alone reads. a has b's request to take."""
    fleet = make(2)
    fleet.a.create(['a', 'b'])
    fleet.a.insert('open', 'o')
    fleet.a.insert('work/w1', '1')
    fleet.a.insert('work/w2', '2')
    fleet.a.set_readers('work/', ['a'])
    settle(fleet.all())
    expect(not fleet.b.can('show', 'work/w1')[0], "b reads work/ before asking")
    fleet.b.set_readers('work/', ['a', 'b'])
    expect(not fleet.b.can('show', 'work/w1')[0], "b reads work/ just by having asked")
    both_sync(fleet)
    return fleet


def s16(make):
    fleet = request(make)
    expect(fleet.a.pending('b') == ['readers work/: a b', 'stale work/w1', 'stale work/w2'],
           f"a is shown {fleet.a.pending('b')}")
    fleet.a.take('b')
    settle(fleet.all())
    check(fleet.all(), {'open': 'o', 'work/w1': '1', 'work/w2': '2'})


def s17(make):
    fleet = request(make)
    for _ in range(2):
        fleet.b.sync()
        fleet.a.sync()
    expect(not fleet.b.can('show', 'work/w1')[0], "b reads work/ although a never took its request")
    expect(fleet.a.readers('work/') == ['a'], "a's readers of work/ changed although it took nothing")


def s18(make):
    fleet = shared_folder(make)
    fleet.a.set_readers('shared/', ['a'])
    fleet.a.insert('shared/s1', '2')
    settle(fleet.all())
    check(fleet.all(), {'private': 'p', 'shared/s1': '2'})


def s19(make):
    for choice, expected in (('mine', ['a', 'b']), ('theirs', ['a', 'b', 'c'])):
        fleet = make(3)
        fleet.a.create(['a', 'b', 'c'])
        fleet.a.insert('shared/s1', '1')
        fleet.a.set_readers('shared/', ['a', 'b', 'c'])
        settle(fleet.all())
        fleet.a.set_readers('shared/', ['a', 'b'])
        fleet.b.set_readers('shared/', ['a', 'b', 'c'])  # the same set, written again: no change
        fleet.b.set_readers('shared/', ['b', 'a', 'c'])
        fleet.b.insert('shared/s2', '2')
        fleet.a.set_readers('shared/', ['a'])
        fleet.a.set_readers('shared/', ['a', 'b'])
        both_sync(fleet)
        pending = fleet.a.pending('b')
        if 'conflict shared/' in pending:
            unchanged(fleet.a, 'take', 'b')
            fleet.a.take('b', **{choice: ['shared/']})
        else:
            # b ended on the set it started from: only a changed the readers, and a's set stands.
            expected = ['a', 'b']
            fleet.a.take('b')
        settle(fleet.all())
        expect(fleet.a.readers('shared/') == expected, f"the readers of shared/ are {fleet.a.readers('shared/')}")
        check(fleet.all(), {'shared/s1': '1', 'shared/s2': '2'})


def s20(make):
    fleet = make(2)
    fleet.a.create(['a', 'b'])
    fleet.a.insert('private/p', '1')
    fleet.a.set_readers('private/', ['a'])
    fleet.a.insert('shared/keep', 'k')
    fleet.a.set_readers('shared/', ['a', 'b'])
    settle(fleet.all())
    expect(not fleet.b.can('show', 'private/p')[0], "b reads private/")
    fleet.a.mv('private/p', 'shared/p')
    settle(fleet.all())
    check(fleet.all(), {'shared/p': '1', 'shared/keep': 'k'})


def s21(make):
    fleet = shared_folder(make, count=3)
    fleet.b.insert('shared/s1', 'B')
    fleet.a.set_readers('shared/', ['a', 'b', 'c'])
    settle(fleet.all())
    check(fleet.all(), {'private': 'p', 'shared/s1': 'B'})


def s22(make):
    fleet = started(make, count=3)
    fleet.a.insert('from-a', 'a')
    fleet.b.insert('from-b', 'b')
    fleet.c.insert('from-c', 'c')
    for device in fleet.all():
        device.sync()
    for taker, source in ((fleet.b, 'a'), (fleet.c, 'b'), (fleet.a, 'c')) * 2:
        if taker.sync().get(source) == 'take':
            taker.take(source)
        taker.sync()
    settle(fleet.all())
    check(fleet.all(), {'seed': 's', 'from-a': 'a', 'from-b': 'b', 'from-c': 'c'})


def s23(make):
    fleet = started(make, count=3)
    only = {'c': ['b']}
    fleet.a.insert('from-a', 'a')
    settle(fleet.all(), only=only)
    fleet.a.mv('from-a', 'renamed')
    fleet.c.insert('from-c', 'c')
    settle(fleet.all(), only=only)
    check(fleet.all(), {'seed': 's', 'renamed': 'a', 'from-c': 'c'}, only=only)


def gone_wrong(make, count=2):
    """a of any kind; b a pass.py device, which the scenario makes publish what it never would."""
    fleet = started(make, count)
    if not isinstance(fleet.b, PyDevice):
        return None
    return fleet


def refused(fleet, source='b'):
    before = fleet.a.ls()
    states = fleet.a.sync()
    expect(states.get(source) == 'refused', f"a did not refuse {source}'s file: {states.get(source + ':why')}")
    expect(fleet.a.ls() == before, "a's entries changed")
    return states[source + ':why']


def s24(make):
    fleet = gone_wrong(make)
    if not fleet:
        return 'skipped: b must be a pass.py device'
    stranger = fleet.b.make_key('stranger')
    with open(fleet.b.readers_file('/'), 'a') as readers:
        readers.write(stranger + '\n')
    fleet.b.git('commit', '--quiet', '-am', 'Let a stranger read everything.')
    expect(not fleet.b.can('passpy', 'sync', 'push')[0], "b's pass.py published a .gpg-id naming a stranger")
    fleet.b.raw_publish()
    refused(fleet)


def s25(make):
    fleet = gone_wrong(make)
    if not fleet:
        return 'skipped: b must be a pass.py device'
    stranger = fleet.b.make_key('stranger')
    fleet.b.gpg('--yes', '--trust-model', 'always', '--encrypt', '--recipient', fleet.a.fingerprint, '--recipient',
                fleet.b.fingerprint, '--recipient', stranger, '--output', os.path.join(fleet.b.store, 'leak.gpg'),
                text_in='copied for a stranger\n')
    fleet.b.git('add', 'leak.gpg')
    fleet.b.git('commit', '--quiet', '-m', 'An entry a stranger reads.')
    expect(not fleet.b.can('passpy', 'sync', 'push')[0], "b's pass.py published an entry encrypted to a stranger")
    fleet.b.raw_publish()
    refused(fleet)


def s26(make):
    def extension(b):
        os.makedirs(os.path.join(b.store, '.extensions'))
        with open(os.path.join(b.store, '.extensions', 'x.bash'), 'w') as script:
            script.write('#!/bin/sh\n')
        b.git('add', '-f', '.extensions')

    def link(b):
        os.symlink('/etc/passwd', os.path.join(b.store, 'link.gpg'))
        b.git('add', 'link.gpg')

    def attributes(b):
        with open(os.path.join(b.store, '.gitattributes'), 'w') as attributes_file:
            attributes_file.write('*.gpg diff=gpg filter=evil\n')
        b.git('add', '.gitattributes')

    def note(b):
        with open(os.path.join(b.store, 'README'), 'w') as text:
            text.write('not an entry\n')
        b.git('add', 'README')

    for wrong in (extension, link, attributes, note):
        fleet = gone_wrong(make)
        if not fleet:
            return 'skipped: b must be a pass.py device'
        wrong(fleet.b)
        fleet.b.git('commit', '--quiet', '-m', 'Something that is no entry.')
        expect(not fleet.b.can('passpy', 'sync', 'push')[0], f"b's pass.py published this: {wrong.__name__}")
        fleet.b.raw_publish()
        refused(fleet)


def s27(make):
    fleet = gone_wrong(make, count=3)
    if not fleet:
        return 'skipped: b must be a pass.py device'
    b = fleet.b
    b.insert('more', 'm')

    b.raw_publish(name='c')  # b's file, signed by b, where c's should be
    refused(fleet, 'c')
    fleet.c.sync()  # c puts its own back

    b.git('branch', '-f', 'second', 'HEAD')
    b.raw_publish(b.seal(b.raw_bundle('refs/pass-sync/b', 'refs/heads/second')))
    refused(fleet)

    whole = b.seal(b.raw_bundle())
    with open(whole, 'rb') as message:
        half = message.read()[:os.path.getsize(whole) // 2]
    with open(whole + '.half', 'wb') as message:
        message.write(half)
    b.raw_publish(whole + '.half')
    refused(fleet)

    b.gpg('--yes', '--trust-model', 'always', '--encrypt', '--recipient', fleet.a.fingerprint,
          '--output', whole + '.unsigned', b.raw_bundle())
    b.raw_publish(whole + '.unsigned')
    refused(fleet)

    b.raw_publish()  # a good one: a receives it
    expect(fleet.a.sync()['b'] == 'take', "a does not see b's good file")
    newest = os.path.join(fleet.zone.files, 'b.bundle.gpg')
    shutil.copy(newest, whole + '.newest')
    b.git('reset', '--quiet', '--hard', 'HEAD~1')
    before = fleet.a.ls()
    b.raw_publish()  # an earlier state of b, served again
    states = fleet.a.sync()
    expect(states['b'] == 'ignored', f"a did not ignore b's old file: {states.get('b:why')}")
    expect(fleet.a.ls() == before, "a's entries changed")
    fleet.a.take('b')
    expect(fleet.a.show('more') == 'm', "a lost what it had received before the old file came")


def s28(make):
    fleet = started(make, count=3)
    fleet.a.insert('x', '1')
    fleet.a.sync()
    fleet.c.sync()
    fleet.c.take('a')
    fleet.c.sync()
    fleet.a.drop('b')
    expect(fleet.a.ls() == ['seed'], f"a did not go back to b's state: {fleet.a.ls()}")
    settle(fleet.all())
    check(fleet.all(), {'seed': 's', 'x': '1'})


def s29(make):
    fleet = make(3)
    fleet.a.create(['a', 'b', 'c'])
    fleet.a.insert('shared/s1', '1')
    fleet.a.set_readers('shared/', ['a', 'b'])
    settle(fleet.all())
    fleet.c.set_readers('shared/', ['a', 'b', 'c'])  # c asks
    fleet.c.sync()
    for device in (fleet.a, fleet.b):  # a and b each grant it, without having seen the other do so
        expect(device.sync()['c'] == 'take', f"{device.name} does not see c's request")
    saved = {}
    for device in (fleet.a, fleet.b):
        device.take('c')
    for device in (fleet.a, fleet.b):
        for line in device.pending('b' if device is fleet.a else 'a') if device.sync() else []:
            expect(not line.startswith('conflict'), f"{device.name} sees a conflict between two re-encryptions: {line}")
    settle(fleet.all())
    check(fleet.all(), {'shared/s1': '1'})
    del saved


def s30(make):
    fleet = started(make, count=3, entries={f'e{number}': str(number) for number in range(4)})
    rng = random.Random(30)
    counter = itertools.count()

    def lower_name_wins(device, other, _name):
        return 'mine' if device.name < other.name else 'theirs'

    for _ in range(20):
        for device in fleet.all():
            for _ in range(rng.randint(1, 2)):
                names = device.ls()
                action = rng.choice(['add', 'add', 'change', 'move', 'remove'] if names else ['add'])
                new = f"{rng.choice(['', 'x/', 'y/'])}n{next(counter)}"
                if action == 'add':
                    device.insert(new, new)
                elif action == 'change':
                    device.insert(rng.choice(names), f'v{next(counter)}')
                elif action == 'move':
                    device.mv(rng.choice(names), new)
                else:
                    device.rm(rng.choice(names))
        if rng.random() < 0.7:
            settle(rng.sample(fleet.all(), rng.randint(2, 3)), resolve=lower_name_wins)
    settle(fleet.all(), resolve=lower_name_wins)
    contents = {name: fleet.a.show(name) for name in fleet.a.ls()}
    check(fleet.all(), contents)


SCENARIOS = {name[1:]: function for name, function in sorted(globals().items())
             if name.startswith('s') and name[1:].isdigit() and callable(function)}


def main(argv):
    if os.geteuid() != 0:
        sys.exit("run.py needs root: it starts the drop zone")
    if not os.path.isfile(PASS_PY):
        sys.exit("set PASS_PY to the path of pass.py")
    mixes, keep, wanted = None, False, []
    while argv:
        argument = argv.pop(0)
        if argument == '--kinds':
            mixes = [tuple(argv.pop(0).split(','))]
        elif argument == '--keep':
            keep = True
        else:
            wanted.append(argument.zfill(2))
    if mixes is None:
        mixes = [mix for mix in itertools.product(KINDS, repeat=3)]
    if any('core' in mix for mix in mixes) and not os.path.isfile(PASS_DEVICE):
        sys.exit(f"no pass-device at {PASS_DEVICE}: cargo build -p pass-core --example pass-device")
    unknown = [number for number in wanted if number not in SCENARIOS]
    if unknown:
        sys.exit(f"no scenario {', '.join(unknown)}")

    # Not under /tmp: sshd refuses to chroot below a directory others can write.
    work = tempfile.mkdtemp(prefix='pass-scenarios.', dir='/var/lib')
    os.chmod(work, 0o755)
    failures, played = [], set()
    try:
        for number in wanted or SCENARIOS:
            for mix in mixes:
                fleets = []

                def make(count, mix=mix, fleets=fleets):
                    fleets.append(Fleet(mix[:count], work))
                    return fleets[-1]

                started_at = time.time()
                try:
                    note = SCENARIOS[number](make)
                    # A scenario with two devices is the same for every third kind: once is enough.
                    used = tuple(mix[:max((len(fleet.devices) for fleet in fleets), default=0)])
                    if (number, used) in played and not note:
                        outcome = None
                    else:
                        played.add((number, used))
                        outcome = note or 'ok'
                except (Failed, CommandFailed) as failure:
                    used = tuple(mix[:max((len(fleet.devices) for fleet in fleets), default=len(mix))])
                    if (number, used, 'failed') in played:
                        outcome = None
                    else:
                        played.add((number, used, 'failed'))
                        outcome = f"FAILED: {failure}"
                        failures.append((number, used))
                finally:
                    for fleet in fleets:
                        fleet.stop()
                        if not keep:
                            shutil.rmtree(fleet.work, ignore_errors=True)
                if outcome:
                    print(f"{number} {','.join(used):14} {time.time() - started_at:5.1f}s  {outcome}", flush=True)
    finally:
        if not keep:
            shutil.rmtree(work, ignore_errors=True)
    if failures:
        sys.exit(f"{len(failures)} failed: " + ' '.join(f"{number}[{','.join(used)}]" for number, used in failures))
    print("all scenarios passed")


if __name__ == '__main__':
    main(sys.argv[1:])
