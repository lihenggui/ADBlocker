# Android translations

## Add a language

1. In every Android module with `src/main/res/values/strings.xml`, add
   `src/main/res/values-<qualifier>/strings.xml`. Use Android resource
   qualifiers. Indonesian uses `values-in`. Use `values-b+zh+Hant` for
   Traditional Chinese across Taiwan, Hong Kong, and Macau. Simplified
   Chinese uses `values-zh`.
2. Translate every default string and plural resource except entries marked
   `translatable="false"`. Use the comments above the English resources
   for context. Review the entries marked `Security:` for precise meaning.
   Keep placeholder indices and conversions, such as `%1$s` and `%2$d`,
   even when changing their order. Use explicit positions for multiple
   arguments and `%%` for a literal percent in a format string. For plural
   quantities absent from English, preserve the arguments in its `other`
   form.
3. Add the qualifier without the `values-` prefix to `appLocales` in
   [android/gradle.properties](../android/gradle.properties). The release
   resource filter and the translation checker both use this list. The
   default `values/` supplies English. For Traditional Chinese, add
   `b+zh+Hant` to `appLocales`. The generated app locale config uses the
   language tag `zh-Hant`.
4. Run `./gradlew test` and `./gradlew :app:lintRelease` from `android/`.
   The JVM test checks coverage, resource types, format arguments, and
   cardinal plural quantities using the same data as Android lint for the
   app's target SDK. The failure names the module and resource to fix.
   Chinese plurals use only `other`.
5. Run `./gradlew :app:generateReleaseLocaleConfig :app:assembleRelease`.
   Check the generated XML under `app/build/generated/res/localeConfig/release/`
   for the new language tag. Check that `aapt2 dump configurations` on the
   release APK lists the new locale. Run `./gradlew :app:checkReleaseBadging`
   and compare `app/release-badging.txt` with
   `app/build/outputs/apk_from_bundle/release/release-badging.txt`.
   If the differences are limited to the new locale, run
   `./gradlew :app:updateReleaseBadging` and rerun the check.

Debug builds include `en-XA` for expanded text and `ar-XB` for right-to-left
layout. Use them to check wrapping and the direction of embedded technical
values.

## Keep unchanged

Keep ADBlocker, HTTPS, VPN, root, LSPosed, Magisk, KernelSU, APatch, EasyList,
AdGuard, and uBlock Origin unchanged. Preserve rule syntax, host names,
URLs, package names, and certificate file names as written.

## Glossary

| English | Meaning in ADBlocker | Simplified Chinese | Traditional Chinese | Russian | Spanish | Portuguese (Brazil) | Indonesian | Vietnamese | Turkish | Japanese | Korean |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| App | An Android application. | 应用 | 應用程式 | Приложение | Aplicación | App | Aplikasi | Ứng dụng | Uygulama | アプリ | 앱 |
| Settings (Android) | Android's system settings. | 设置 | 設定 | Настройки | Ajustes | Configurações | Setelan | Cài đặt | Ayarlar | 設定 | 설정 |
| Network | The network an app connects to. | 网络 | 網路 | Сеть | Red | Rede | Jaringan | Mạng | Ağ | ネットワーク | 네트워크 |
| Block | Refuse a request or a connection that matches a blocking rule. | 拦截 | 封鎖 | Блокировать | Bloquear | Bloquear | Blokir | Chặn | Engelle | ブロック | 차단 |
| Allow | Let a request or connection through. | 放行 | 允許 | Разрешить | Permitir | Permitir | Izinkan | Cho phép | İzin ver | 許可 | 허용 |
| Trust | Let connections to a site bypass blocking and decryption. | 信任 | 信任 | Доверять | Confiar (sitios de confianza) | Confiar | Percayai | Tin cậy | Güven | 信頼 | 신뢰 |
| Exclude | Bypass filtering for an app in the All apps scope. | 排除 | 排除 | Исключить | Excluir | Excluir | Kecualikan | Loại trừ | Hariç tut | 除外 | 제외 |
| Pause | Temporarily suspend filtering globally or for an app. | 暂停 | 暫停 | Приостановить | Pausar | Pausar | Jeda | Tạm dừng | Duraklat | 一時停止 | 일시중지 |
| Resume | End a filtering pause. | 恢复 | 繼續 | Возобновить | Reanudar | Retomar | Lanjutkan | Lọc tiếp | Devam et | 再開 | 재개 |
| Filter / filtering | Apply the enabled lists and custom rules to eligible traffic. | 过滤 | 過濾 | Фильтровать / фильтрация | Filtrar / filtrado | Filtrar / filtragem | Saring / penyaringan | Lọc | Filtrele / filtreleme | フィルタリング | 필터링 |
| List / Lists tab | A subscription that supplies filter rules. The Lists tab manages subscriptions. | 过滤列表（标签页为“订阅”） | 過濾清單（分頁標題為「訂閱」） | Список фильтров / Списки | Lista de filtros (pestaña «Listas») | Lista de filtros (aba “Listas”) | Daftar filter (tab “Daftar”) | Danh sách lọc (thẻ “Danh sách”) | Liste / Listeler | フィルタリスト（タブ名は「リスト」） | 필터 목록 / 구독 |
| Rule | A directive that matches traffic or page elements, or defines an exception. | 规则 | 規則 | Правило | Regla | Regra | Aturan | Quy tắc | Kural | ルール | 규칙 |
| Page element hiding | Hide elements named by rules on decrypted pages. | 隐藏页面元素 | 隱藏網頁元素 | Скрытие элементов страниц | Ocultar elementos de página | Ocultar elementos da página | Sembunyikan elemen halaman | Ẩn phần tử trang | Sayfa öğelerini gizleme | ページ要素を非表示 | 페이지 요소 숨기기 |
| URL filtering | Apply request-level rules to URLs, including inside HTTPS where supported. | 按网址过滤 | 依網址過濾 | Фильтрация по URL | Filtrado por URL | Filtragem de URL | Penyaringan URL | Lọc theo URL | URL filtreleme | URL フィルタリング | URL 필터링 |
| Certificate | The local CA certificate that an app must trust for HTTPS inspection. | 证书 | 憑證 | Сертификат | Certificado | Certificado | Sertifikat | Chứng chỉ | Sertifika | 証明書 | 인증서 |
| User certificates / CA certificate | Android's user-installed certificates and a certificate authority's certificate. | 用户证书 / CA 证书 | 使用者憑證 / CA 憑證 | Пользовательские сертификаты / сертификат центра сертификации | Certificados de usuario / Certificado AC | Certificados do usuário / Certificado de CA | Sertifikat pengguna / sertifikat CA | Chứng chỉ người dùng / Chứng chỉ CA | Kullanıcı sertifikaları / CA sertifikası | ユーザー証明書 / CA 証明書 | 사용자 인증서 / CA 인증서 |
| All apps | Filter eligible personal apps, with exclusions. Work apps are selected separately. | 全部应用 | 所有應用程式 | Все приложения | Todas las aplicaciones | Todos os apps | Semua aplikasi | Tất cả ứng dụng | Tüm uygulamalar | すべてのアプリ | 모든 앱 |
| Chosen apps | Filter apps the user explicitly selects. | 选定应用 | 選定的應用程式 | Выбранные приложения | Aplicaciones seleccionadas | Apps selecionados | Aplikasi pilihan | Ứng dụng đã chọn | Seçilen uygulamalar | 選択したアプリ | 선택한 앱 |
| Work profile | The managed Android profile whose apps and certificate trust are separate. | 工作资料 | 工作資料夾 | Рабочий профиль | Perfil de trabajo | Perfil de trabalho | Profil kerja | Hồ sơ công việc | İş profili | 仕事用プロファイル | 직장 프로필 |
| Root mode | Use the on-device root component to filter traffic. | root 模式 | root 模式 | Режим root | Modo root | Modo root | Mode root | Chế độ root | Root modu | root モード | root 모드 |
| VPN mode | Use a local Android VPN to filter traffic without root. | VPN 模式 | VPN 模式 | Режим VPN | Modo VPN | Modo VPN | Mode VPN | Chế độ VPN | VPN modu | VPN モード | VPN 모드 |
| Default | The setting in effect until the user changes it. | 默认 | 預設 | По умолчанию | Predeterminado | Padrão | Default | Mặc định | Varsayılan | デフォルト | 기본값 |
| File / save | A document such as a rules export or certificate, or the action of saving it. | 文件 / 保存 | 檔案 / 儲存 | Файл / сохранить | Archivo / guardar | Arquivo / salvar | File / simpan | Tệp / Lưu | Dosya / kaydet | ファイル / 保存 | 파일 / 저장 |
| Storage | Space where Android keeps files or app data. | 私有目录 | 儲存空間 | Хранилище | Almacenamiento | Armazenamento | Penyimpanan | Bộ nhớ | Depolama | ストレージ | 저장용량 |
| Notification | A message shown by Android in the notification shade. | 通知 | 通知 | Уведомление | Notificación | Notificação | Notifikasi | Thông báo | Bildirim | 通知 | 알림 |
| Update (verb) | Download a fresh list or install a newer version of root mode. | 更新 | 更新 | Обновить | Actualizar | Atualizar | Perbarui | Cập nhật | Güncelle | 更新 | 업데이트 |
| Log / record | Operational logs or the action of recording network requests. | 日志 / 记录 | 記錄 | Журнал / записывать | Registro / registrar | Registro / gravar | Log / rekam | Nhật ký / Ghi | Günlük / kaydet | ログ / 記録 | 로그 / 기록 |
