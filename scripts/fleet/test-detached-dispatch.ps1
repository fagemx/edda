# GH605 real Windows Job test. Uses only disposable fixture processes/tasks.
param([Parameter(Mandatory)][string]$Edda, [string]$BaselineEdda = '',
  [ValidateSet('worker-1','worker-2')][string]$BuildLane = 'worker-2')
$ErrorActionPreference = 'Stop'
$Edda = (Resolve-Path -LiteralPath $Edda).Path
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('edda-detach-test-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $testRoot)
$pwsh = (Get-Process -Id $PID).Path
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class DetachJobTest {
 [StructLayout(LayoutKind.Sequential)] public struct Basic { public long a,b; public uint flags; public UIntPtr min,max; public uint count; public UIntPtr affinity; public uint priority,scheduling; }
 [StructLayout(LayoutKind.Sequential)] public struct Io { public ulong a,b,c,d,e,f; }
 [StructLayout(LayoutKind.Sequential)] public struct Extended { public Basic basic; public Io io; public UIntPtr a,b,c,d; }
 [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr CreateJobObject(IntPtr security, string name);
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetInformationJobObject(IntPtr job, int kind, ref Extended info, uint size);
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
 [DllImport("kernel32.dll", SetLastError=true)] public static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool result);
 [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr handle);
 public static IntPtr Create() { var j=CreateJobObject(IntPtr.Zero,null); var e=new Extended(); e.basic.flags=0x2000; if(j==IntPtr.Zero || !SetInformationJobObject(j,9,ref e,(uint)Marshal.SizeOf(e))) throw new Exception("job creation failed"); return j; }
}
'@
function Assert([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Wait-Until([scriptblock]$Condition, [string]$Message, [int]$Seconds=60) {
  $until=(Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $until) { if (& $Condition) { return }; Start-Sleep -Milliseconds 200 }
  throw $Message
}
function Tick-Count([string]$Path) { if(Test-Path -LiteralPath $Path){return @(Get-Content -LiteralPath $Path).Count}; return 0 }
function Quote([string]$Value) { return "'" + $Value.Replace("'","''") + "'" }
$taskNames = [Collections.Generic.List[string]]::new()
$controllers = [Collections.Generic.List[Diagnostics.Process]]::new()
$jobs = [Collections.Generic.List[IntPtr]]::new()
$supervisorHelper = $null
try {
  $laneRoot=if($env:FLEET_LANE_ROOT){$env:FLEET_LANE_ROOT}else{Join-Path $env:LOCALAPPDATA 'fleet-workstation/lanes'}
  $fixtureBuild=Join-Path (Join-Path $laneRoot $BuildLane) 'gh605-fixtures'
  [void](New-Item -ItemType Directory -Force -Path $fixtureBuild)
  $stubSource=Join-Path $testRoot 'fake-launcher.rs'
  Set-Content -LiteralPath $stubSource -Value @'
fn main() {
 let args: Vec<_> = std::env::args_os().skip(1).collect();
 if args.iter().any(|a| a == "--version") { println!("fixture 1"); return; }
 let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
 let shell = std::fs::read_to_string(dir.join("pwsh-path.txt")).unwrap();
 let status = std::process::Command::new(shell.trim()).arg("-NoProfile").arg("-File")
   .arg(dir.join("fake.ps1")).args(args).status().unwrap();
 std::process::exit(status.code().unwrap_or(1));
}
'@
  & rustc --crate-name gh605_fake_launcher $stubSource -o (Join-Path $fixtureBuild 'claude.exe')
  Assert ($LASTEXITCODE -eq 0) 'fake backend compiler failed'
  $cases=@(
    @{name='detached';exe=$Edda;detach=$true;timeout=40;build=$true;expectTimeout=$false},
    @{name='timeout';exe=$Edda;detach=$true;timeout=5;build=$false;expectTimeout=$true}
  )
  if($BaselineEdda){
    $cases=@(@{name='baseline';exe=(Resolve-Path $BaselineEdda).Path;detach=$false;timeout=40;build=$false;expectTimeout=$false})+$cases
  }
  foreach($case in $cases) {
    $dir=Join-Path $testRoot $case.name
    [void](New-Item -ItemType Directory -Path $dir)
    $ticks=Join-Path $dir 'ticks.log'; $agentId=Join-Path $dir 'agent.pid'
    $envFile=Join-Path $dir 'environment.json'; $fakePs=Join-Path $dir 'fake.ps1'
    $tiny=Join-Path $dir 'tiny'; [void](New-Item -ItemType Directory -Path (Join-Path $tiny 'src'))
    Set-Content -LiteralPath (Join-Path $tiny 'Cargo.toml') -Value "[package]`nname='edda_detach_fixture'`nversion='0.0.0'`nedition='2021'`n[workspace]"
    Set-Content -LiteralPath (Join-Path $tiny 'src/main.rs') -Value 'fn main() {}'
    $agentTemplate=@'
if($args -contains '--version'){ 'fixture 1'; exit 0 }
Set-Content -LiteralPath __PID__ -Value $PID
@{home=$env:HOME;target=$env:CARGO_TARGET_DIR;cwd=(Get-Location).Path} | ConvertTo-Json | Set-Content -LiteralPath __ENV__
if (__BUILD__) { & cargo build --quiet --manifest-path __CARGO__; if($LASTEXITCODE -ne 0){exit $LASTEXITCODE} }
for($i=0;$i -lt 80;$i++) { Add-Content -LiteralPath __TICKS__ -Value $i; Start-Sleep -Milliseconds 250 }
'{"type":"result","subtype":"success","is_error":false,"result":"fixture complete","duration_ms":20000,"num_turns":1,"session_id":"fixture"}'
'@
    $agentTemplate=$agentTemplate.Replace('__PID__',(Quote $agentId)).Replace('__ENV__',(Quote $envFile)).Replace('__TICKS__',(Quote $ticks)).Replace('__CARGO__',(Quote (Join-Path $tiny 'Cargo.toml'))).Replace('__BUILD__',$(if($case.build){'$true'}else{'$false'}))
    Set-Content -LiteralPath $fakePs -Value $agentTemplate
    $fake=Join-Path $dir 'claude.exe'
    Copy-Item -LiteralPath (Join-Path $fixtureBuild 'claude.exe') -Destination $fake
    Set-Content -LiteralPath (Join-Path $dir 'pwsh-path.txt') -Value $pwsh
    Set-Content -LiteralPath (Join-Path $dir 'prompt.txt') -Value 'fixture'
    $go=Join-Path $dir 'go'; $output=Join-Path $dir 'launch.json'; $controllerScript=Join-Path $dir 'controller.ps1'
    $control=@'
while(-not(Test-Path -LiteralPath __GO__)){Start-Sleep -Milliseconds 100}
$env:PATH=__CWD__ + ';' + $env:PATH
$env:EDDA_STORE_ROOT=__STORE__
$env:HOME='poison-home'
$env:CARGO_TARGET_DIR='poison-target'
    & __EDDA__ dispatch --agent claude --prompt-file __PROMPT__ --cwd __CWD__ --timeout-sec __TIMEOUT__ --json __DETACH__ __OWNS__ > __OUTPUT__
Start-Sleep -Seconds 120
'@
    $detachArgs=if($case.detach){'--detach --build-lane '+$BuildLane+' --detach-log-dir '+(Quote (Join-Path $dir 'logs'))}else{''}
    $ownsArgs=if($case.expectTimeout){'--owns crates/fixture/timeout'}else{''}
    $control=$control.Replace('__GO__',(Quote $go)).Replace('__FAKE__',(Quote $fake)).Replace('__STORE__',(Quote (Join-Path $dir 'store'))).Replace('__EDDA__',(Quote $case.exe)).Replace('__PROMPT__',(Quote (Join-Path $dir 'prompt.txt'))).Replace('__CWD__',(Quote $dir)).Replace('__DETACH__',$detachArgs).Replace('__OWNS__',$ownsArgs).Replace('__TIMEOUT__',$case.timeout).Replace('__OUTPUT__',(Quote $output))
    Set-Content -LiteralPath $controllerScript -Value $control
    $controller=Start-Process -FilePath $pwsh -ArgumentList @('-NoProfile','-File',('"'+$controllerScript+'"')) -WindowStyle Hidden -PassThru
    $controllers.Add($controller)
    $job=[DetachJobTest]::Create(); $jobs.Add($job)
    Assert ([DetachJobTest]::AssignProcessToJobObject($job,$controller.Handle)) 'controller must enter kill-on-close job'
    Set-Content -LiteralPath $go -Value go
    $minimumTicks=if($case.expectTimeout){1}else{3}
    Wait-Until { (Tick-Count $ticks) -ge $minimumTicks } "agent never wrote ticks: $dir"
    $agent=Get-Process -Id ([int](Get-Content -LiteralPath $agentId))
    $inside=$false
    Assert ([DetachJobTest]::IsProcessInJob($agent.Handle,$job,[ref]$inside)) 'IsProcessInJob failed'
    Assert ($inside -eq (-not $case.detach)) "wrong Job membership: detached=$($case.detach), inside=$inside"
    if($case.detach) {
      $receipt=Get-Content -Raw -LiteralPath $output | ConvertFrom-Json
      $taskNames.Add($receipt.task)
      $scheduled = Get-ScheduledTask -TaskName $receipt.task -ErrorAction Stop
      Assert ($null -ne $scheduled) 'detached task was not registered'
      $supervisorHelper = $receipt.manifest -replace '\.json$','.task.ps1'
      $wrapper = Get-Content -Raw -LiteralPath $supervisorHelper
      $launchConfig = Get-Content -Raw -LiteralPath ($receipt.manifest -replace '\.json$','.launch.json') | ConvertFrom-Json
      Assert ($wrapper -match '(?m)^# lane-reap: controller-pid=\d+ controller-started=.+$') 'generated wrapper must expose canonical reaper metadata'
      $metadataPattern = '(?m)^# lane-reap: controller-pid=' + [regex]::Escape([string]$launchConfig.controller_pid) + '\s'
      Assert ($wrapper -match $metadataPattern) 'wrapper reaper metadata has the wrong controller PID'
      $envSeen=Get-Content -Raw -LiteralPath $envFile | ConvertFrom-Json
      $laneRoot=if($env:FLEET_LANE_ROOT){$env:FLEET_LANE_ROOT}else{Join-Path $env:LOCALAPPDATA 'fleet-workstation/lanes'}
      $expectedTarget=Join-Path $laneRoot $BuildLane
      Assert ($envSeen.home -eq $env:USERPROFILE) 'HOME must be explicitly restored'
      Assert ($envSeen.target -eq $expectedTarget) 'named Cargo lane was not forwarded'
      if($case.build){
        Assert (Test-Path -LiteralPath (Join-Path $expectedTarget 'debug/edda_detach_fixture.exe')) 'fixture build did not use assigned lane'
      }
      Assert (-not(Test-Path -LiteralPath (Join-Path $dir 'target'))) 'worktree target must not be created'
    }
    [void][DetachJobTest]::CloseHandle($job); [void]$jobs.Remove($job)
    $before=Tick-Count $ticks
    Start-Sleep -Seconds 2
    $after=Tick-Count $ticks
    if($case.detach -and -not $case.expectTimeout){Assert ($after -gt $before) 'detached agent stopped with controller'}
    elseif(-not $case.detach) {Assert ($after -eq $before) 'baseline should die with its controller Job'}
    "PASS $($case.name): job_inside=$inside ticks_before=$before ticks_after=$after"
    if($case.detach) {
      Wait-Until {
        if(Get-ScheduledTask -TaskName $receipt.task -ErrorAction SilentlyContinue){return $false}
        $terminal=Get-Content -Raw -LiteralPath $receipt.manifest | ConvertFrom-Json
        return $terminal.state -in @('completed','timeout','failed')
      } 'task was not unregistered with a terminal manifest' 70
      $manifest=Get-Content -Raw -LiteralPath $receipt.manifest | ConvertFrom-Json
      if($case.expectTimeout){
        Assert ($manifest.state -eq 'timeout' -and $manifest.exit_code -eq 2) 'timeout did not terminate the worker'
        $oldStore=$env:EDDA_STORE_ROOT
        try {
          $env:EDDA_STORE_ROOT=Join-Path $dir 'store'
          & $Edda claim check 'crates/fixture/timeout'
          Assert ($LASTEXITCODE -eq 0) 'timeout left its owned claim active'
        } finally {
          if($null -eq $oldStore){$env:EDDA_STORE_ROOT=$null}
          else{$env:EDDA_STORE_ROOT=$oldStore}
        }
        "PASS timeout: handle=$($receipt.handle) task_absent=true claim_released=true state=$($manifest.state)"
      } else {
        Assert ($manifest.state -eq 'completed' -and $manifest.exit_code -eq 0) 'terminal manifest missing or unsuccessful'
        Assert ((Get-Item -LiteralPath $receipt.log).Length -gt 0) 'returned log path has no result'
        "PASS completion: handle=$($receipt.handle) task_absent=true state=$($manifest.state)"
      }
    }
  }
  # Force only terminal persistence to fail after an owned scheduled task has
  # started. The helper must still unregister that exact task and exit nonzero.
  $terminalDir=Join-Path $testRoot 'terminal-manifest-failure'; [void](New-Item -ItemType Directory -Path $terminalDir)
  $terminalManifest=Join-Path $terminalDir 'manifest.json'; $terminalConfig=Join-Path $terminalDir 'launch.json'
  $terminalTask='edda-gh605-terminal-'+[guid]::NewGuid().ToString('N')
  $terminalAction=New-ScheduledTaskAction -Execute $pwsh -Argument '-NoProfile -Command exit 0'
  Register-ScheduledTask -TaskName $terminalTask -Action $terminalAction | Out-Null
  $terminal=@{manifest=$terminalManifest;log=(Join-Path $terminalDir 'worker.log');task=$terminalTask;cwd=$terminalDir;executable=$Edda;argv=@('--version');environment=@{};home=$env:USERPROFILE;cargo=$null;timeout=20;session='cli-terminal';owned_paths=@();test_fail_terminal_manifest_write=$true}
  @{state='launching';worker_pid=$null;exit_code=$null;error=$null} | ConvertTo-Json | Set-Content -LiteralPath $terminalManifest -Encoding utf8
  $terminal | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $terminalConfig -Encoding utf8
  & $supervisorHelper -Mode Run -Config $terminalConfig
  Assert ($LASTEXITCODE -ne 0) 'terminal manifest write failure must return a nonzero supervisor result'
  Assert ($null -eq (Get-ScheduledTask -TaskName $terminalTask -ErrorAction SilentlyContinue)) 'terminal manifest failure left its task registered'
  "PASS terminal-manifest-failure: task_absent=true exit=$LASTEXITCODE"
} catch {
  # Keep the failing assertion beside the retained fixture evidence.  A
  # detached process can outlive this harness, so terminal output alone is
  # not a durable failure receipt.
  $_ | Out-File -LiteralPath (Join-Path $testRoot 'failure.txt') -Encoding utf8
  throw
} finally {
  foreach($job in $jobs){[void][DetachJobTest]::CloseHandle($job)}
  foreach($controller in $controllers){if(-not $controller.HasExited){$controller.Kill($true)}}
  foreach($name in $taskNames){if(Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue){Stop-ScheduledTask -TaskName $name; Unregister-ScheduledTask -TaskName $name -Confirm:$false}}
  # Retain fixtures and logs for the acceptance receipt; no worktree deletion.
  "evidence=$testRoot"
}
