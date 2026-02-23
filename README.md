# TrustTunnel

TrustTunnel — это библиотека для построения VPN-туннеля поверх HTTPS (HTTP/1.1, HTTP/2) и QUIC (HTTP/3).

## Назначение

- туннелирование TCP/UDP-трафика через TLS/HTTPS/QUIC;
- маскировка трафика под обычный веб-трафик;
- серверная интеграция с несколькими режимами аутентификации.

## Короткая схема конфигурации

1. Настроить `settings.toml` (сетевые параметры, `auth.mode`, протоколы, опционально `metrics`).
2. Настроить `hosts.toml` (TLS-хосты и сертификаты).
3. Для Basic/Mixed создать `credentials.toml`.
4. Для JWT/Mixed заполнить `[auth.jwt]`.

### Включение смешанного режима (Mixed)

1. Создайте `credentials.toml` с пользователями для Basic-аутентификации.
2. В `settings.toml` задайте:
   - `auth.mode = "mixed"`
   - секцию `[auth.jwt]` (алгоритм, issuer/audience при необходимости, claims, ключ/secret).
3. После этого сервер принимает одновременно:
   - `Authorization: Bearer <token>`
   - `Authorization`/`Proxy-Authorization: Basic ...`

## Документация

- Подробная конфигурация: [CONFIGURATION.md](CONFIGURATION.md)
- Описание протокола: [PROTOCOL.md](PROTOCOL.md)
