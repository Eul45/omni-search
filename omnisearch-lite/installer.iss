[Setup]
AppName=OmniSearch Lite
AppVersion=1.0.0
DefaultDirName={localappdata}\Programs\omnisearch-lite
DefaultGroupName=OmniSearch Lite
UninstallDisplayIcon={app}\omnisearch-lite.exe
SetupIconFile=icons\OmniSearchTrans.ico
Compression=lzma2
SolidCompression=yes
OutputDir=setup
OutputBaseFilename=omnisearchlitesetup
PrivilegesRequired=lowest
CloseApplications=yes
RestartApplications=no

[Files]
Source: "target\release\omnisearch-lite.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "target\release\uninstall.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\OmniSearch Lite"; Filename: "{app}\omnisearch-lite.exe"
Name: "{userdesktop}\OmniSearch Lite"; Filename: "{app}\omnisearch-lite.exe"
Name: "{group}\Uninstall OmniSearch Lite"; Filename: "{app}\uninstall.exe"

[Run]
Filename: "{app}\omnisearch-lite.exe"; Description: "Launch OmniSearch Lite"; Flags: nowait postinstall

[UninstallRun]
Filename: "taskkill"; Parameters: "/F /IM omnisearch-lite.exe"; Flags: runhidden; RunOnceId: "KillApp"

[UninstallDelete]
Type: filesandordirs; Name: "{userappdata}\omnisearch-lite"

[Code]
// Guarantee the running app is closed right before file replacement.
procedure TerminateApp;
var
  ResultCode: Integer;
  AppPath: String;
  BackupPath: String;
  I: Integer;
  Renamed: Boolean;
begin
  // Force kill all possible process names using full path to taskkill
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM omnisearch-lite.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM omnisearch-lite.bak', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM omnisearch.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  
  Sleep(500); // let processes terminate

  // Rename fallback for omnisearch-lite.exe
  AppPath := ExpandConstant('{app}\omnisearch-lite.exe');
  if FileExists(AppPath) then
  begin
    Renamed := False;
    // Try standard .bak first
    BackupPath := ExpandConstant('{app}\omnisearch-lite.bak');
    DeleteFile(BackupPath);
    if RenameFile(AppPath, BackupPath) then
    begin
      Renamed := True;
    end else begin
      // Try unique names .bak1, .bak2 ... if standard .bak is locked
      for I := 1 to 5 do
      begin
        BackupPath := ExpandConstant('{app}\omnisearch-lite.bak' + IntToStr(I));
        DeleteFile(BackupPath);
        if RenameFile(AppPath, BackupPath) then
        begin
          Renamed := True;
          Break;
        end;
      end;
    end;

    if Renamed then
      Log('Successfully renamed locked omnisearch-lite.exe')
    else
      Log('Failed to rename locked omnisearch-lite.exe');
  end;
end;

procedure RemoveLegacyOmniSearchInstall;
var
  LegacyInstallDir: String;
  LegacyProgramsDir: String;
  ResultCode: Integer;
begin
  // Remove the old OmniSearch application install while keeping %APPDATA%\omnisearch intact.
  LegacyInstallDir := ExpandConstant('{localappdata}\Programs\omnisearch');
  if DirExists(LegacyInstallDir) then
  begin
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM omnisearch.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    DelTree(LegacyInstallDir, True, True, True);
    Log('Removed legacy OmniSearch install directory: ' + LegacyInstallDir);
  end;

  DeleteFile(ExpandConstant('{userdesktop}\omnisearch.lnk'));
  LegacyProgramsDir := ExpandConstant('{userprograms}\omnisearch');
  if DirExists(LegacyProgramsDir) then
  begin
    DelTree(LegacyProgramsDir, True, True, True);
    Log('Removed legacy OmniSearch Start Menu folder: ' + LegacyProgramsDir);
  end;

  RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\omnisearch_is1');
  RegDeleteKeyIncludingSubkeys(HKLM, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\omnisearch_is1');
end;

// PrepareToInstall runs just before the file copy, so the exe is guaranteed free by then.
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  NeedsRestart := False;
  TerminateApp;
  RemoveLegacyOmniSearchInstall;
  Result := '';
end;
