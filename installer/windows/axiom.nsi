; Axiom 1.3.1 Windows installer. Built by tools/release/build-windows.ps1 or CI.
Unicode true
RequestExecutionLevel user
Name "Axiom"
OutFile "Axiom-Setup-1.3.1.exe"
InstallDir "$LOCALAPPDATA\Programs\Axiom"
InstallDirRegKey HKCU "Software\Axiom" "InstallDir"
ShowInstDetails show
ShowUninstDetails show

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Axiom browser" Main
  SetOutPath "$INSTDIR"
  File "..\..\dist\Axiom-1.3.1-windows-x86_64\axiom.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Axiom"
  CreateShortCut "$SMPROGRAMS\Axiom\Axiom.lnk" "$INSTDIR\axiom.exe"
  CreateShortCut "$DESKTOP\Axiom.lnk" "$INSTDIR\axiom.exe"
  WriteRegStr HKCU "Software\Axiom" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Axiom" "DisplayName" "Axiom 1.3.1"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Axiom" "DisplayVersion" "1.3.1"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Axiom" "UninstallString" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\axiom.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Axiom\Axiom.lnk"
  RMDir "$SMPROGRAMS\Axiom"
  Delete "$DESKTOP\Axiom.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Axiom"
  DeleteRegKey HKCU "Software\Axiom"
SectionEnd
