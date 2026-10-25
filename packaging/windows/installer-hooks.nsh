; Nehir kaldırılırken uygulamanın çalışırken yazdığı kullanıcı girdilerini de temizler.
; (Kurulum klasörü ve indirme listesi/ayarlar silinmez; kullanıcı verisi kullanıcıya aittir.)

!macro NSIS_HOOK_POSTUNINSTALL
  ; Native messaging: tarayıcıların kayıt defteri anahtarları (uygulama her açılışta yeniden yazar).
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\org.nehir.host"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\org.nehir.host"
  DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\org.nehir.host"
  DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\org.nehir.host"
  DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\org.nehir.host"
  ; "Windows ile başlat" ayarı
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Nehir"
  ; Manifest dosyaları (artık var olmayan host'u gösterirlerdi)
  RMDir /r "$LOCALAPPDATA\nehir\native-messaging"
!macroend
