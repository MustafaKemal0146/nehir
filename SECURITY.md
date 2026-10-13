# Güvenlik politikası

## Bir açık bulduysan

Lütfen herkese açık issue **açma**. Bunun yerine GitHub'ın özel bildirim yolunu kullan:
**Security → Report a vulnerability** (`https://github.com/MustafaKemal0146/nehir/security/advisories/new`).

Bildirimde mümkünse sürümü, adımları ve etkiyi yaz. Mümkün olan en kısa sürede dönüş yapılır.

## Tasarım ilkeleri

- Yerel köprü yalnızca `127.0.0.1`'de dinler ve yalnızca tarayıcı eklentilerinden gelen istekleri kabul eder (`Host` denetimi, eklenti kökeni ya da özel başlık).
- Eklenti, indirmenin kaydedileceği klasörü seçemez.
- Native messaging host yalnızca "uygulamayı başlat" komutunu işler; URL ve çerez o yoldan geçmez.
- Çerezler yalnızca ilgili indirme için kullanılır; sunucu değişen yönlendirmelerde gönderilmez.
- Ayar ve kayıt dosyaları `0600` izniyle yazılır.

## Kapsam dışı

DRM korumasının atlatılması bu projenin hedefi değildir.
