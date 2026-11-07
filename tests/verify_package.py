import os, subprocess, tempfile, zipfile
from pathlib import Path
root=Path(__file__).resolve().parents[1]
with zipfile.ZipFile(root/'dist/colmsg-windows-x64.zip') as archive:
    assert archive.testzip() is None
    assert set(archive.namelist()) == {'colmsg.exe','colmsg-ui.cmd','vcruntime140.dll','README.txt','LICENSE.txt','RUNTIME-NOTICE.txt'}
    with tempfile.TemporaryDirectory(prefix='package-smoke-',dir=root/'dist') as temp:
        archive.extractall(temp)
        temp=Path(temp)
        result=subprocess.run([str(temp/'colmsg.exe'),'--version'],check=True,capture_output=True,text=True)
        print('Packaged executable:',result.stdout.strip())
        env=os.environ.copy();env['COLMSG_SMOKE_ROOT']=str(temp)
        command="""$ErrorActionPreference='Stop'; $path=Join-Path $env:COLMSG_SMOKE_ROOT 'colmsg-ui.cmd'; $raw=[IO.File]::ReadAllText($path); $marker='### POWERSHELL_UI_BELOW ###'; $source=$raw.Substring($raw.LastIndexOf($marker)+$marker.Length); $errors=$null; $tokens=$null; $ast=[System.Management.Automation.Language.Parser]::ParseInput($source,[ref]$tokens,[ref]$errors); if($errors.Count){throw 'Invalid embedded UI'}; $function=$ast.Find({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Resolve-DefaultExePath'},$true); Invoke-Expression $function.Extent.Text; $script:root=$env:COLMSG_SMOKE_ROOT; if((Resolve-DefaultExePath) -ne (Join-Path $script:root 'colmsg.exe')){throw 'Wrong packaged executable'}; Write-Output 'PASS: packaged UI resolves the adjacent executable'"""
        subprocess.run(['powershell.exe','-NoProfile','-Command',command],env=env,check=True)
        env['COLMSG_TEST_EXE']=str(temp/'colmsg.exe')
        subprocess.run(['python',str(root/'tests/verify_release_pagination.py')],env=env,check=True)
print('PASS: portable ZIP structure, executable, UI resolution, and pagination')
