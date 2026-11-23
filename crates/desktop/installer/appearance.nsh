!searchreplace SETUP_ART "${HEADERIMAGE}" "installer-header.bmp" ""
!define SETUP_SURFACE "F7F9FD"
!define SETUP_INK "13264A"
!define SETUP_MUTED "4B5D78"

Var SetupBitmap
Var SetupBodyFont
Var SetupLabelFont
Var SetupProgressText

!macro SetupControl HWND ROLE FONT
  SendMessage ${HWND} ${WM_SETFONT} ${FONT} 1
  System::Call '"$PLUGINSDIR\skin.dll"::ApplyControl(p${HWND},i${ROLE})v ?c'
!macroend

!macro SetupMove HWND X Y WIDTH HEIGHT
  Push $4
  Push $5
  Push $6
  Push $7
  Push $8
  System::Call '*(i ${X},i ${Y},i ${WIDTH},i ${HEIGHT})p.r4'
  System::Call 'user32::MapDialogRect(p$HWNDPARENT,p r4)'
  System::Call '*$4(i.r5,i.r6,i.r7,i.r8)'
  System::Free $4
  System::Call 'user32::SetWindowPos(p${HWND},p0,i r5,i r6,i r7,i r8,i 0x14)'
  Pop $8
  Pop $7
  Pop $6
  Pop $5
  Pop $4
!macroend

!macro SetupColors HWND TEXT BG
  ${IfNot} ${IsHighContrastModeActive}
    SetCtlColors ${HWND} "${TEXT}" "${BG}"
  ${EndIf}
!macroend

!macro SetupHero PAGE TEXT ASSET
  ${IfNot} ${IsHighContrastModeActive}
    InitPluginsDir
    File "/oname=$PLUGINSDIR\${ASSET}.bmp" "${SETUP_ART}\${ASSET}.bmp"
    System::Call 'user32::CreateWindowExW(i0,w"STATIC",w"${TEXT}",i0x5400000E,i0,i0,i0,i0,p${PAGE},p0,p0,p0)p.r1'
    !insertmacro SetupMove $1 0 0 330 110
    System::Call '"$PLUGINSDIR\skin.dll"::FillClientWidth(p$HWNDPARENT,p${PAGE})v ?c'
    System::Call '"$PLUGINSDIR\skin.dll"::FillClientWidth(p${PAGE},p$1)v ?c'
    ${NSD_SetStretchedBitmap} $1 "$PLUGINSDIR\${ASSET}.bmp" $SetupBitmap
  ${Else}
    System::Call 'user32::CreateWindowExW(i0,w"STATIC",w"${TEXT}",i0x50000000,i0,i0,i0,i0,p${PAGE},p0,p0,p0)p.r1'
    !insertmacro SetupMove $1 14 47 302 24
    SendMessage $1 ${WM_SETFONT} $SetupLabelFont 1
  ${EndIf}
!macroend

Function SetupStyleInit
  InitPluginsDir
  File "/oname=$PLUGINSDIR\skin.dll" "${SETUP_ART}\skin.dll"
  System::Call '"$PLUGINSDIR\skin.dll"::SetupCaption(p$HWNDPARENT)v ?c'
  CreateFont $SetupBodyFont "Microsoft YaHei UI" "9" "400"
  CreateFont $SetupLabelFont "Microsoft YaHei UI" "10" "700"
  !insertmacro SetupControl $mui.Button.Next 1 $SetupLabelFont
  !insertmacro SetupControl $mui.Button.Cancel 2 $SetupBodyFont
  !insertmacro SetupControl $mui.Button.Back 2 $SetupBodyFont
  !insertmacro SetupMove $mui.Button.Next 246 199 70 21
  !insertmacro SetupMove $mui.Button.Cancel 174 199 64 21
  !insertmacro SetupMove $mui.Button.Back 102 199 64 21
  !insertmacro SetupColors $HWNDPARENT "" "${SETUP_SURFACE}"
  GetDlgItem $0 $HWNDPARENT 1034
  !insertmacro SetupColors $0 "" "${SETUP_SURFACE}"
  !insertmacro SetupColors $mui.Header.Text "${SETUP_INK}" "${SETUP_SURFACE}"
  !insertmacro SetupColors $mui.Header.SubText "${SETUP_MUTED}" "${SETUP_SURFACE}"
  !insertmacro SetupMove $mui.Header.Text 14 22 218 11
  !insertmacro SetupMove $mui.Header.SubText 14 34 218 13
FunctionEnd

!macro SetupCaption PAGE FINISHING
  System::Call '"$PLUGINSDIR\skin.dll"::PositionCaption(p$HWNDPARENT,p${PAGE},i${FINISHING})v ?c'
!macroend

Function SetupStyleExit
  Call SetupFreeImage
  System::Call 'gdi32::DeleteObject(p$SetupBodyFont)'
  System::Call 'gdi32::DeleteObject(p$SetupLabelFont)'
  System::Call 'kernel32::GetModuleHandleW(w"$PLUGINSDIR\skin.dll")p.r0'
  ${If} $0 P<> 0
    System::Call 'kernel32::FreeLibrary(pr0)'
  ${EndIf}
FunctionEnd

Function SetupFreeImage
  ${NSD_FreeImage} $SetupBitmap
  StrCpy $SetupBitmap 0
FunctionEnd

Function SetupExpand
  Pop $0
  Call muiPageLoadFullWindow
  !insertmacro SetupMove $0 0 0 330 192
  !insertmacro SetupColors $0 "" "${SETUP_SURFACE}"
FunctionEnd

Function SetupMaintenanceShow
  Pop $3
  Pop $2
  Pop $9
  Call SetupFreeImage
  Call SetupExpand
  !insertmacro SetupHero $0 "管理已有安装" "maintenance"
  !insertmacro SetupMove $9 14 117 302 24
  SendMessage $9 ${WM_SETFONT} $SetupBodyFont 1
  !insertmacro SetupColors $9 "${SETUP_MUTED}" "${SETUP_SURFACE}"
  !insertmacro SetupMove $2 14 144 302 16
  !insertmacro SetupMove $3 14 165 302 16
  !insertmacro SetupControl $2 7 $SetupBodyFont
  !insertmacro SetupControl $3 7 $SetupBodyFont
  SendMessage $mui.Button.Next ${WM_SETTEXT} 0 "STR:继续"
  SendMessage $mui.Button.Cancel ${WM_SETTEXT} 0 "STR:取消"
  !insertmacro SetupCaption $0 0
FunctionEnd

Function SetupDirectoryShow
  Call SetupFreeImage
  Push $mui.DirectoryPage
  Call SetupExpand
  !insertmacro SetupHero $mui.DirectoryPage "安装知乎答主助手" "setup"
  !insertmacro SetupMove $mui.DirectoryPage.Text 14 119 302 12
  SendMessage $mui.DirectoryPage.Text ${WM_SETTEXT} 0 "STR:安装位置"
  SendMessage $mui.DirectoryPage.Text ${WM_SETFONT} $SetupLabelFont 1
  !insertmacro SetupColors $mui.DirectoryPage.Text "${SETUP_INK}" "${SETUP_SURFACE}"
  ShowWindow $mui.DirectoryPage.DirectoryBox ${SW_HIDE}
  !insertmacro SetupMove $mui.DirectoryPage.Directory 14 136 242 17
  !insertmacro SetupControl $mui.DirectoryPage.Directory 3 $SetupBodyFont
  !insertmacro SetupColors $mui.DirectoryPage.Directory "${SETUP_INK}" "FFFFFF"
  !insertmacro SetupMove $mui.DirectoryPage.BrowseButton 262 135 54 19
  SendMessage $mui.DirectoryPage.BrowseButton ${WM_SETTEXT} 0 "STR:浏览…"
  !insertmacro SetupControl $mui.DirectoryPage.BrowseButton 2 $SetupBodyFont
  !insertmacro SetupMove $mui.DirectoryPage.SpaceRequired 14 160 302 11
  !insertmacro SetupMove $mui.DirectoryPage.SpaceAvailable 14 174 302 11
  SendMessage $mui.DirectoryPage.SpaceRequired ${WM_SETFONT} $SetupBodyFont 1
  SendMessage $mui.DirectoryPage.SpaceAvailable ${WM_SETFONT} $SetupBodyFont 1
  !insertmacro SetupColors $mui.DirectoryPage.SpaceRequired "${SETUP_MUTED}" "${SETUP_SURFACE}"
  !insertmacro SetupColors $mui.DirectoryPage.SpaceAvailable "${SETUP_MUTED}" "${SETUP_SURFACE}"
  SendMessage $mui.Button.Next ${WM_SETTEXT} 0 "STR:立即安装"
  SendMessage $mui.Button.Cancel ${WM_SETTEXT} 0 "STR:取消"
  SendMessage $mui.Button.Back ${WM_SETTEXT} 0 "STR:上一步"
  ${If} $SetupCanGoBack != 1
    ShowWindow $mui.Button.Back ${SW_HIDE}
  ${EndIf}
  SendMessage $mui.DirectoryPage.Directory ${EM_SETSEL} -1 0
  ${NSD_SetFocus} $mui.Button.Next
  !insertmacro SetupCaption $mui.DirectoryPage 0
FunctionEnd

Function SetupDirectoryLeave
  Call muiPageUnloadFullWindow
FunctionEnd

Function SetupInstallationShow
  Call SetupFreeImage
  Push $mui.InstFilesPage
  Call SetupExpand
  !insertmacro SetupHero $mui.InstFilesPage "正在安装" "progress"
  ShowWindow $mui.InstFilesPage.Text ${SW_HIDE}
  System::Call 'user32::CreateWindowExW(i0,w"STATIC",w"正在写入应用文件…",i0x50000000,i0,i0,i0,i0,p$mui.InstFilesPage,p0,p0,p0)p.s'
  Pop $SetupProgressText
  !insertmacro SetupMove $SetupProgressText 14 118 302 12
  SendMessage $SetupProgressText ${WM_SETFONT} $SetupBodyFont 1
  !insertmacro SetupColors $SetupProgressText "${SETUP_INK}" "${SETUP_SURFACE}"
  !insertmacro SetupMove $mui.InstFilesPage.ProgressBar 14 133 302 9
  ${IfNot} ${IsHighContrastModeActive}
    System::Call 'uxtheme::SetWindowTheme(p$mui.InstFilesPage.ProgressBar,w" ",w" ")'
    SendMessage $mui.InstFilesPage.ProgressBar ${PBM_SETBARCOLOR} 0 0xE76117
    SendMessage $mui.InstFilesPage.ProgressBar ${PBM_SETBKCOLOR} 0 0xF0E6D8
  ${EndIf}
  !insertmacro SetupMove $mui.InstFilesPage.ShowLogButton 14 151 65 17
  !insertmacro SetupControl $mui.InstFilesPage.ShowLogButton 2 $SetupBodyFont
  SendMessage $mui.InstFilesPage.ShowLogButton ${WM_SETTEXT} 0 "STR:显示详情"
  !insertmacro SetupMove $mui.InstFilesPage.Log 14 149 302 38
  ShowWindow $mui.Button.Back ${SW_HIDE}
  ShowWindow $mui.Button.Next ${SW_HIDE}
  ShowWindow $mui.Button.Cancel ${SW_HIDE}
  !insertmacro SetupCaption $mui.InstFilesPage 0
FunctionEnd

Function SetupInstallationLeave
  Call muiPageUnloadFullWindow
FunctionEnd

Function SetupInstallationFailed
  ${If} $SetupProgressText P<> 0
    SendMessage $SetupProgressText ${WM_SETTEXT} 0 "STR:安装未完成。可打开详情查看原因。"
    SendMessage $mui.Button.Cancel ${WM_SETTEXT} 0 "STR:关闭"
    ShowWindow $mui.Button.Cancel ${SW_SHOW}
    EnableWindow $mui.Button.Cancel 1
  ${EndIf}
FunctionEnd

Function SetupFinishShow
  Call SetupFreeImage
  ShowWindow $mui.FinishPage.Image ${SW_HIDE}
  ShowWindow $mui.FinishPage.Title ${SW_HIDE}
  !insertmacro SetupColors $mui.FinishPage "" "${SETUP_SURFACE}"
  !insertmacro SetupHero $mui.FinishPage "安装完成" "finish"
  !insertmacro SetupMove $mui.FinishPage.Text 14 121 302 18
  SendMessage $mui.FinishPage.Text ${WM_SETFONT} $SetupBodyFont 1
  SendMessage $mui.FinishPage.Text ${WM_SETTEXT} 0 "STR:选择完成后要执行的操作。"
  !insertmacro SetupColors $mui.FinishPage.Text "${SETUP_INK}" "${SETUP_SURFACE}"
  !insertmacro SetupMove $mui.FinishPage.Run 14 143 302 16
  !insertmacro SetupMove $mui.FinishPage.ShowReadme 14 165 302 16
  !insertmacro SetupControl $mui.FinishPage.Run 4 $SetupBodyFont
  !insertmacro SetupControl $mui.FinishPage.ShowReadme 4 $SetupBodyFont
  !insertmacro SetupColors $mui.FinishPage.Run "${SETUP_INK}" "${SETUP_SURFACE}"
  !insertmacro SetupColors $mui.FinishPage.ShowReadme "${SETUP_INK}" "${SETUP_SURFACE}"
  ${If} ${RebootFlag}
    SendMessage $mui.FinishPage.Text ${WM_SETTEXT} 0 "STR:需要重启 Windows 才能完成安装。"
    !insertmacro SetupMove $mui.FinishPage.RebootNow 14 143 302 16
    !insertmacro SetupMove $mui.FinishPage.RebootLater 14 165 302 16
  ${EndIf}
  ShowWindow $mui.Button.Back ${SW_HIDE}
  ShowWindow $mui.Button.Cancel ${SW_HIDE}
  ShowWindow $mui.Button.Next ${SW_SHOW}
  SendMessage $mui.Button.Next ${WM_SETTEXT} 0 "STR:完成"
  !insertmacro SetupCaption $mui.FinishPage 1
FunctionEnd
