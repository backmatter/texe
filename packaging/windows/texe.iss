; Paths and version are supplied by package-windows-app.ps1.
[Setup]
AppId={{5D637AAF-6BB6-4CAE-8BAC-9736CC0EA6C8}
AppName=texe
AppVersion={#Version}
AppPublisher=Backmatter
AppPublisherURL=https://github.com/backmatter/texe
DefaultDirName={localappdata}\Programs\texe-desktop
DefaultGroupName=texe
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
OutputDir={#OutputDir}
OutputBaseFilename=texe-x86_64-windows-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\texe-desktop.exe
CloseApplications=yes
#ifdef SignCommand
SignTool=release $f
SignedUninstaller=yes
#endif

[Files]
Source: "{#Stage}\texe-desktop.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\bin\*"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\PDFJS-LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\texe"; Filename: "{app}\texe-desktop.exe"

[Run]
Filename: "{app}\texe-desktop.exe"; Description: "Open texe and create your first paper"; Flags: nowait postinstall skipifsilent
