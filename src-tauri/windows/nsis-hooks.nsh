!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegKey HKCU "Software\Classes\*\shell\OmniSearch.SendToPhone"
  DeleteRegKey HKCU "Software\Classes\AllFilesystemObjects\shell\OmniSearch.SendToPhone"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\OmniSearch.SendToPhone"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\OmniSearch.SendToPhone"
!macroend
