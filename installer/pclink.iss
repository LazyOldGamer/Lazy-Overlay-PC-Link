; PC Link's installer (Inno Setup 7): the plain download, for this user
; only (no admin rights), with a Start menu entry, an uninstaller and,
; when ticked, starting with Windows and keeping running by the clock.
; Settings are kept on uninstall.
;
;   iscc /DVersion=<version> /DExe=<lazy-overlay-link.exe> /O<out> pclink.iss

#ifndef Version
  #error Pass /DVersion=<version>
#endif
#ifndef Exe
  #define Exe "..\target\release\lazy-overlay-link.exe"
#endif

[Setup]
; Never change AppId: it ties new versions to this program.
AppId={{6F1D2A57-3C4E-4B8A-9E2F-5A7C1B3D9E04}
AppName=Lazy Overlay PC Link
AppVersion={#Version}
AppVerName=Lazy Overlay PC Link {#Version}
AppPublisher=LazyOldGamer
VersionInfoVersion={#Version}
VersionInfoProductName=Lazy Overlay PC Link
VersionInfoDescription=Lazy Overlay PC Link Setup
VersionInfoCompany=LazyOldGamer
VersionInfoCopyright=LazyOldGamer
PrivilegesRequired=lowest
DefaultDirName={autopf}\Lazy Overlay PC Link
DefaultGroupName=Lazy Overlay PC Link
DisableProgramGroupPage=yes
DisableDirPage=auto
OutputBaseFilename=lazy-overlay-link-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\lazy-overlay-link.exe
UninstallDisplayName=Lazy Overlay PC Link
WizardStyle=modern
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/max
SolidCompression=yes
; No build time stored: the same program gives the same bytes.
TimeStampsInUTC=yes
TouchDate=2020-01-01
TouchTime=00:00:00
; PC Link is closed before its file is replaced.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "startup"; Description: "Start PC Link when Windows starts (minimized)"; Flags: unchecked
Name: "tray"; Description: "Keep PC Link running by the clock when its window is closed"; Flags: unchecked
Name: "desktopicon"; Description: "Add a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#Exe}"; DestDir: "{app}"; DestName: "lazy-overlay-link.exe"; Flags: ignoreversion

[Icons]
Name: "{group}\Lazy Overlay PC Link"; Filename: "{app}\lazy-overlay-link.exe"
Name: "{userdesktop}\Lazy Overlay PC Link"; Filename: "{app}\lazy-overlay-link.exe"; Tasks: desktopicon

[Registry]
; The same value as the window's checkbox (src/autostart.rs).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Lazy Overlay PC Link"; ValueData: """{app}\lazy-overlay-link.exe"" --minimized"; Tasks: startup
; Removed on uninstall however it was set.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Lazy Overlay PC Link"; Flags: uninsdeletevalue

[Run]
; Saved by PC Link itself, in its settings file, as the window's checkbox.
Filename: "{app}\lazy-overlay-link.exe"; Parameters: "--close-to-tray yes"; Flags: runhidden waituntilterminated; Tasks: tray
Filename: "{app}\lazy-overlay-link.exe"; Description: "Open PC Link now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Asked to close, so its file can go.
Filename: "{sys}\taskkill.exe"; Parameters: "/IM lazy-overlay-link.exe"; Flags: runhidden; RunOnceId: "ClosePcLink"
