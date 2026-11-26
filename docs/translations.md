# Android translations

## Add a language

1. In every Android module with `src/main/res/values/strings.xml`, add
   `src/main/res/values-<qualifier>/strings.xml`. Use Android resource
   qualifiers. Indonesian uses `values-in`. Traditional Chinese uses
   `values-zh-rTW` for Taiwan or `values-b+zh+Hant` for the Traditional
   script across regions.
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
   default `values/` supplies English.
4. Run `./gradlew test` and `./gradlew :app:lintRelease` from `android/`.
   The JVM test checks coverage, resource types, format arguments, and
   cardinal plural quantities from ICU4J's CLDR data. The failure names
   the module and resource to fix.

Debug builds include `en-XA` for expanded text and `ar-XB` for right-to-left
layout. Use them to check wrapping and the direction of embedded technical
values.

## Keep unchanged

Keep ADBlocker, HTTPS, VPN, root, LSPosed, Magisk, KernelSU, APatch, EasyList,
AdGuard, and uBlock Origin unchanged. Preserve rule syntax, host names,
URLs, package names, and certificate file names as written.

## Glossary

| English | Meaning in ADBlocker | Simplified Chinese |
| --- | --- | --- |
| Block | Refuse a request or a connection that matches a blocking rule. | 拦截 |
| Allow | Let a request or connection through. | 放行 |
| Trust | Let connections to a site bypass blocking and decryption. | 信任 |
| Exclude | Bypass filtering for an app in the All apps scope. | 排除 |
| Pause | Temporarily suspend filtering globally or for an app. | 暂停 |
| Filter | Apply the enabled lists and custom rules to eligible traffic. | 过滤 |
| List | A subscription that supplies filter rules. The Lists tab manages subscriptions. | 过滤列表（标签页为“订阅”） |
| Rule | A directive that matches traffic or page elements, or defines an exception. | 规则 |
| Page element hiding | Hide elements named by rules on decrypted pages. | 隐藏页面元素 |
| URL filtering | Apply request-level rules to URLs, including inside HTTPS where supported. | 按网址过滤 |
| Certificate | The local CA certificate that an app must trust for HTTPS inspection. | 证书 |
| All apps | Filter eligible personal apps, with exclusions. Work apps are selected separately. | 全部应用 |
| Chosen apps | Filter apps the user explicitly selects. | 选定应用 |
| Work profile | The managed Android profile whose apps and certificate trust are separate. | 工作资料 |
| Root mode | Use the on-device root component to filter traffic. | root 模式 |
| VPN mode | Use a local Android VPN to filter traffic without root. | VPN 模式 |
