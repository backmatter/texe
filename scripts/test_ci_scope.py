import importlib.util, json, os, subprocess, tempfile
from pathlib import Path
from unittest.mock import patch
p=Path(__file__).with_name('ci-scope.py')
import sys
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('scope',p);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
scratch=tempfile.TemporaryDirectory(prefix='ci-reuse-');original_cwd=Path.cwd();root=Path(scratch.name);os.chdir(root)
def git(*a):return subprocess.check_output(['git',*a],text=True,stderr=subprocess.PIPE).strip()
git('init','-q');git('config','user.name','CI regression');git('config','user.email','ci@example.invalid');git('config','commit.gpgsign','false')
(root/'Cargo.toml').write_text('[package]\nname="test-tool"\nversion="1.0.0"\n[dependencies]\nserde="1"\n')
(root/'Cargo.lock').write_text('version=4\n[[package]]\nname="test-tool"\nversion="1.0.0"\n[[package]]\nname="serde"\nversion="1.0.0"\nsource="registry"\n')
(root/'CHANGELOG.md').write_text('Existing release\n');(root/'source.rs').write_text('fn main() {}\n')
git('add','.');git('commit','-qm','base');base=git('rev-parse','HEAD')
(root/'Cargo.toml').write_text((root/'Cargo.toml').read_text().replace('version="1.0.0"','version="1.0.1"'))
(root/'Cargo.lock').write_text((root/'Cargo.lock').read_text().replace('name="test-tool"\nversion="1.0.0"','name="test-tool"\nversion="1.0.1"'))
(root/'CHANGELOG.md').write_text('New release\n');git('add','.');git('commit','-qm','version');release=git('rev-parse','HEAD')
assert m.metadata_only(base,release);print('PASS version and changelog changes qualify')
for path,content in [('source.rs','fn main() { panic!(); }\n'),('Cargo.toml',(root/'Cargo.toml').read_text().replace('serde="1"','serde="2"')),('Cargo.lock',(root/'Cargo.lock').read_text().replace('name="serde"\nversion="1.0.0"','name="serde"\nversion="2.0.0"'))]:
 git('reset','--hard',release);(root/path).write_text(content);git('add','.');git('commit','-qm','change')
 assert not m.metadata_only(base,'HEAD'),path
 print('PASS source or dependency change requires full validation:',path)
git('reset','--hard',release)
os.environ['GITHUB_REPOSITORY']='owner/repo'
pr={'merged_at':'now','merge_commit_sha':base,'head':{'sha':'a'*40}}
run={'id':1,'event':'pull_request','head_repository':{'full_name':'owner/repo'},'html_url':'https://example.invalid/full-check'}
def api(path):
 if path.endswith('/pulls'):return [pr]
 if '/workflows/' in path:return {'workflow_runs':[] if ('head_sha='+base) in path else [run]}
 return {'jobs':[{'name':'Rust (Linux)','conclusion':'success'},{'name':'Rust (Windows)','conclusion':'success'},{'name':'Rust (macOS)','conclusion':'success'}]}
def command(*args):
 if args[:2]==('git','fetch'):return ''
 if len(args)>2 and args[1]=='rev-parse' and args[2].endswith('^{tree}'):return 'same-tree'
 return git(*args[1:])
with patch.object(m,'api',side_effect=api),patch.object(m,'command',side_effect=command):
 assert m.checked_base(base,'ci.yml')==run['html_url'];print('PASS matching successful source check can be reused')
 with patch.object(m,'api',side_effect=lambda p: {'jobs':[{'name':'Rust (Windows)','conclusion':'skipped'}]} if '/jobs?' in p else api(p)):
  assert m.checked_base(base,'ci.yml') is None;print('PASS skipped validation cannot be reused')
 with patch.dict(run,head_repository={'full_name':'foreign/repo'}):
  assert m.checked_base(base,'ci.yml') is None;print('PASS another repository cannot supply validation proof')
 with patch.dict(pr,merge_commit_sha='b'*40):
  assert m.checked_base(base,'ci.yml') is None;print('PASS validation for another base cannot be reused')
# The bot's single final release check may reuse proof; an ordinary manual
# check must still execute the full platform validation.
release_pr={'base': {'sha': base, 'ref': 'main'}, 'head': {'sha': release}}
output=root/'manual-output'
environment={'GITHUB_EVENT_NAME':'workflow_dispatch','GITHUB_SHA':release,'CI_RELEASE_PR':'12','GITHUB_OUTPUT':str(output),'GITHUB_STEP_SUMMARY':str(root/'summary')}
with patch.dict(os.environ,environment),patch.object(m,'api',return_value=release_pr),patch.object(m,'checked_base',return_value=run['html_url']),patch('sys.argv',['ci-scope.py','ci.yml']):
 m.main();assert output.read_text()=='full=false\n';print('PASS synchronized release request can reuse verified source')
 output.unlink()
 with patch.dict(os.environ,CI_RELEASE_PR=''):
  m.main();assert output.read_text()=='full=true\n';print('PASS ordinary manual validation always runs fully')
 output.unlink()
 with patch.dict(release_pr,head={'sha':'b'*40}):
  m.main();assert output.read_text()=='full=true\n';print('PASS a mismatched release head cannot reuse validation')
 output.unlink()
 with patch.object(m,'checked_base',side_effect=subprocess.CalledProcessError(1,['gh','api'])):
  m.main();assert output.read_text()=='full=true\n';print('PASS unavailable validation proof requires full validation')
print('All reuse boundaries passed')

os.chdir(original_cwd)
scratch.cleanup()
