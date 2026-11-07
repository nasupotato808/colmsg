import json, os, subprocess, tempfile, threading
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit, parse_qs

root = Path(__file__).resolve().parents[1]
release_exe = root / 'target/release/colmsg.exe'
exe = release_exe if release_exe.is_file() else root / 'colmsg.exe'
exe = Path(os.environ['COLMSG_TEST_EXE']) if os.environ.get('COLMSG_TEST_EXE') else exe
calls = []
def message(i):
    return dict(id=i, group_id=48, is_favorite=False, is_silent=False,
        published_at='2026-01-01T00:00:00Z', updated_at='2026-01-01T00:00:00Z',
        state='published', type='text', text='Pagination regression test')
def page(ids, cursor, comments=0):
    return dict(messages=[message(i) for i in ids], letters=[], comments=[{}]*comments,
        queried_at='2026-11-07T00:00:00Z', continuation=cursor)
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_GET(self):
        u=urlsplit(self.path); q=parse_qs(u.query)
        if u.path.endswith('/past_messages'): data={'messages':[]}
        elif u.path.endswith('/timeline'):
            cursor=q.get('continuation',['initial'])[0]; calls.append(cursor)
            if cursor=='initial': data=page(range(1,91),'cursor-1',10)
            elif cursor=='cursor-1': data=page(range(0),'cursor-2',100)
            elif cursor=='cursor-2': data=page(range(91,191),'cursor-3')
            elif cursor=='cursor-3': data=page(range(191,252),None)
            else: self.send_error(400); return
            if cursor!='initial':
                assert not any(k in q for k in ('updated_from','count','order'))
        else: self.send_error(404); return
        payload=json.dumps(data).encode()
        self.send_response(200); self.send_header('Content-Type','application/json')
        self.send_header('Content-Length',str(len(payload))); self.end_headers()
        self.wfile.write(payload)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='verify-release-',dir=root/'tests') as temp:
        temp=Path(temp); config=temp/'config'; config.write_text('')
        env=os.environ.copy();env['COLMSG_CONFIG_PATH']=str(config)
        env['S_BASE_URL']='http://127.0.0.1:'+str(server.server_port)
        result=subprocess.run([str(exe),'--s_access_token','mock-token','--message-group-id','48',
            '-n','test','-d',str(temp/'downloads'),'-k','text'],env=env,capture_output=True,
            text=True,encoding='utf-8',timeout=60)
        if result.returncode: raise AssertionError(result.stderr)
        saved=len(list((temp/'downloads'/'test').glob('*.txt')))
        assert saved==251, (saved,result.stdout)
        assert calls==['initial','cursor-1','cursor-2','cursor-3'],calls
        print('PASS: rebuilt release executable saved all 251 messages across 4 pages, including a message-empty page')
        print(result.stdout)
finally:
    server.shutdown();server.server_close();thread.join()
