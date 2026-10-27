"""Exercise the firmware-ota-gate shell logic over success/failure/skip/cancel cases."""
import pathlib, subprocess, sys
import yaml
wf=yaml.safe_load((pathlib.Path(__file__).resolve().parents[1]/'workflows'/'firmware-ota.yml').open())
script=wf['jobs']['firmware-ota-gate']['steps'][0]['run']
base=dict(DISCOVER='success',FORMAT='success',HOST='success',C3='success',S3='success',HAS_HOST='true',HAS_C3='true',HAS_S3='true',EVENT='pull_request',HAS_JOBS='true')
def run(**o):
    e={**base,**o}
    return subprocess.run(['bash','-c',script],env={'PATH':'/usr/bin:/bin',**e},capture_output=True,text=True).returncode
cases=[
 ('all green',{},0),
 ('host test failure',dict(HOST='failure'),1),
 ('MCU compile failure',dict(S3='failure'),1),
 ('selected job skipped by mistake',dict(C3='skipped'),1),
 ('job cancelled',dict(C3='cancelled'),1),
 ('discovery failure',dict(DISCOVER='failure',HAS_HOST='',HAS_C3='',HAS_S3='',HAS_JOBS=''),1),
 ('discovery outputs missing',dict(HAS_HOST=''),1),
 ('format failure',dict(FORMAT='failure'),1),
 ('empty matrix on PR (nothing affected)',dict(HAS_HOST='false',HAS_C3='false',HAS_S3='false',HAS_JOBS='false',HOST='skipped',C3='skipped',S3='skipped'),0),
 ('empty matrix on schedule',dict(HAS_HOST='false',HAS_C3='false',HAS_S3='false',HAS_JOBS='false',HOST='skipped',C3='skipped',S3='skipped',EVENT='schedule'),1),
 ('empty matrix on manual dispatch',dict(HAS_HOST='false',HAS_C3='false',HAS_S3='false',HAS_JOBS='false',HOST='skipped',C3='skipped',S3='skipped',EVENT='workflow_dispatch'),1),
 ('unselected job unexpectedly failed',dict(HAS_S3='false',S3='failure'),1),
 ('host only, MCU skipped',dict(HAS_C3='false',HAS_S3='false',C3='skipped',S3='skipped'),0),
]
bad=0
for n,o,w in cases:
    g=run(**o); ok=(g==w); bad+=not ok
    print(('ok  ' if ok else 'FAIL'),n,'-> exit',g)
sys.exit(bad)
