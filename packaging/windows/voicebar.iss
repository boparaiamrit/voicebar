; Voicebar for Windows: a per-user installer (no admin prompt) built with Inno Setup 6.
;
;   iscc /DAppVersion=0.1.0 /DOutputName=voicebar-v0.1.0-windows-x64-setup packaging\windows\voicebar.iss
;
; Installs voicebar.exe to %LOCALAPPDATA%\voicebar\bin and adds a Start menu shortcut.
; Uninstalling removes voicebar.exe and shortcuts while keeping downloaded models in %LOCALAPPDATA%\voicebar\models.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef OutputName
  #define OutputName "voicebar-setup"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif

[Setup]
AppId={{8D4E6C19-3A27-4F92-9B14-6E5C1D8F7A23}
AppName=Voicebar
AppVersion={#AppVersion}
AppVerName=Voicebar {#AppVersion}
AppPublisher=Amritpal Singh
AppPublisherURL=https://github.com/boparaiamrit/voicebar
AppSupportURL=https://github.com/boparaiamrit/voicebar/issues
AppUpdatesURL=https://github.com/boparaiamrit/voicebar/releases
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Voicebar setup
DefaultDirName={localappdata}\voicebar\bin
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename={#OutputName}
SetupIconFile=..\..\assets\voicebar.ico
UninstallDisplayIcon={app}\voicebar.exe
UninstallDisplayName=Voicebar
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=force
RestartApplications=no

[Messages]
WelcomeLabel2=This installs Voicebar {#AppVersion}: a floating, always-on-top local GPU voice dictation bar powered by NVIDIA Nemotron and Parakeet.%n%nPress F1 from any window to dictate live on your GPU and auto-paste straight into your active input. No administrator rights are needed.

[Tasks]
Name: "autostart"; Description: "Start Voicebar when Windows signs in (preloads GPU silently)"; Flags: checkedonce
Name: "desktopicon"; Description: "Add a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceDir}\voicebar.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Voicebar"; Filename: "{app}\voicebar.exe"; Comment: "Local GPU real-time voice dictation"
Name: "{autodesktop}\Voicebar"; Filename: "{app}\voicebar.exe"; Comment: "Local GPU real-time voice dictation"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Voicebar"; ValueData: """{app}\voicebar.exe"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\voicebar.exe"; Description: "Launch Voicebar in the system tray"; Flags: postinstall nowait skipifsilent

[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec('taskkill.exe', '/F /IM voicebar.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    MsgBox('Voicebar has been removed. Downloaded models and settings remain in ' +
      ExpandConstant('{localappdata}\voicebar') + '. Delete that folder to remove them too.', mbInformation, MB_OK);
end;
