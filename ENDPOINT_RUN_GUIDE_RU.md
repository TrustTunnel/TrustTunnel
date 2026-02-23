# TrustTunnel Endpoint: подробный запуск, аргументы и переменные окружения

Этот гайд — практическая инструкция по запуску `trusttunnel_endpoint`:
- что нужно подготовить;
- какие аргументы CLI есть у бинарника;
- какие переменные окружения нужны (в т.ч. для JWT/HS256);
- как запускать в «новом режиме» через JWT и mixed auth;
- как генерировать клиентский конфиг/deeplink.

> Примеры ниже предполагают, что вы уже собрали бинарник и находитесь в директории с `trusttunnel_endpoint`.

---

## 1) Базовый запуск endpoint

Минимальная команда:

```bash
./trusttunnel_endpoint vpn.toml hosts.toml
```

Где:
- `vpn.toml` — основной конфиг endpoint;
- `hosts.toml` — TLS-хосты и сертификаты.

---

## 2) Все аргументы `trusttunnel_endpoint`

### Обязательные positional аргументы

1. `<settings>`
   - путь к основному конфигу (обычно `vpn.toml`)
2. `<tls_hosts_settings>`
   - путь к TLS hosts конфигу (обычно `hosts.toml`)

Они обязательны всегда, кроме режима `--version`.

### Флаги и опции

- `-v`, `--version`
  - печатает только версию и завершает работу.

- `-l`, `--loglvl <info|debug|trace>`
  - уровень логирования;
  - по умолчанию: `info`.

- `--logfile <path>`
  - путь к файлу логов;
  - если не указан — логи идут в stdout.

- `--sentry_dsn <dsn>`
  - DSN для отправки ошибок в Sentry.

- `--jobs <N>`
  - количество worker threads runtime;
  - если не задано — берётся число CPU.

#### Режим генерации клиентского конфига

- `-c`, `--client_config <client_name>`
  - генерирует конфиг клиента для указанного пользователя и завершает работу.

- `-a`, `--address <ip|ip:port>`
  - адрес endpoint, который попадёт в клиентский конфиг;
  - можно указывать несколько раз;
  - обязателен вместе с `-c`.

- `-s`, `--custom-sni <hostname>`
  - переопределяет SNI для клиента;
  - значение должно совпадать с `hostname` или `allowed_sni` в `hosts.toml`.

- `-r`, `--client-random-prefix <hex>`
  - hex-префикс TLS ClientHello random;
  - должен соответствовать правилу в `rules.toml`, иначе endpoint выведет warning и поле проигнорирует.

- `-f`, `--format <deeplink|toml>`
  - формат генерируемого клиентского конфига;
  - по умолчанию: `deeplink`.

---

## 3) Основные режимы авторизации (auth.mode)

В `vpn.toml`:

```toml
[auth]
mode = "credentials" # credentials | jwt | mixed
```

### `credentials`
- endpoint проверяет логин/пароль по `credentials_file`.

### `jwt` (часто это и называют «новый режим»)
- endpoint ожидает JWT-токен (вместо обычного пароля в Basic auth);
- обязательно заполнить секцию `[auth.jwt]`;
- по умолчанию JWT должен содержать непустой claim `device_id` (настраивается через `device_id_claim`).

### `mixed`
- endpoint принимает и обычные credentials, и JWT.

---

## 4) Какие переменные окружения нужны

### Обязательные только для JWT с `HS256`

Если в `[auth.jwt]`:

```toml
algorithm = "HS256"
hmac_secret_env = "TRUSTTUNNEL_JWT_SECRET"
```

тогда **до запуска endpoint** нужно выставить переменную окружения:

```bash
export TRUSTTUNNEL_JWT_SECRET='очень_длинный_секрет'
./trusttunnel_endpoint vpn.toml hosts.toml
```

Без этой переменной JWT-проверка не сможет корректно работать.

### Для `RS256`

Переменная окружения не требуется, нужен путь к публичному ключу:

```toml
algorithm = "RS256"
public_key_path = "jwt/public.pem"
```

---

## 5) Готовые сценарии запуска

### A. Обычный production-запуск

```bash
./trusttunnel_endpoint vpn.toml hosts.toml --logfile /var/log/trusttunnel.log -l info
```

### B. Debug-запуск

```bash
./trusttunnel_endpoint vpn.toml hosts.toml -l debug
```

### C. JWT/HS256 («новый режим»)

```bash
export TRUSTTUNNEL_JWT_SECRET='replace_with_strong_secret'
./trusttunnel_endpoint vpn.toml hosts.toml -l info
```

### D. Генерация deeplink для клиента

```bash
./trusttunnel_endpoint vpn.toml hosts.toml -c alice -a 203.0.113.10 --format deeplink
```

### E. Генерация TOML-конфига клиента

```bash
./trusttunnel_endpoint vpn.toml hosts.toml -c alice -a 203.0.113.10:443 --format toml
```

### F. Несколько адресов для отказоустойчивости

```bash
./trusttunnel_endpoint vpn.toml hosts.toml -c alice \
  -a 203.0.113.10:443 \
  -a 198.51.100.25:443 \
  --format deeplink
```

---

## 6) Пример `vpn.toml` для JWT режима

```toml
listen_address = "0.0.0.0:443"
credentials_file = "credentials.toml"

[auth]
mode = "jwt"

[auth.jwt]
algorithm = "HS256"
issuer = "https://issuer.example"
audience = "trusttunnel"
leeway_seconds = 30
username_claim = "sub"
device_id_claim = "device_id"
hmac_secret_env = "TRUSTTUNNEL_JWT_SECRET"
```

---

## 7) «Новый режим» через setup_wizard (non-interactive)

Если под «новым режимом» вы имели в виду автоматическую настройку **клиента** (а не endpoint), используйте:

```bash
./setup_wizard --mode non-interactive \
  --endpoint_config <endpoint_config> \
  --settings trusttunnel_client.toml
```

- `--endpoint_config` — файл, который сгенерировал endpoint (`--format toml`) или соответствующий endpoint-конфиг;
- `--mode non-interactive` — запуск без вопросов в консоли.

---

## 8) Частые ошибки

1. **`-c` без `-a`**
   - endpoint не сгенерирует клиентский конфиг: адрес обязателен.

2. **`--custom-sni` не совпадает с `hosts.toml`**
   - endpoint завершится с ошибкой валидации SNI.

3. **`--client-random-prefix` не hex**
   - endpoint завершится с ошибкой формата.

4. **`HS256` без переменной `hmac_secret_env`**
   - JWT авторизация будет некорректной.

5. **Неправильные пути к cert/key в `hosts.toml`**
   - endpoint не поднимется из-за TLS-конфига.

---

## 9) Быстрая памятка

- Запуск endpoint: `./trusttunnel_endpoint vpn.toml hosts.toml`
- Новый auth-режим: `mode = "jwt"` или `mode = "mixed"`
- Для JWT HS256: экспортируйте секрет через env (`hmac_secret_env`)
- Генерация клиентского конфига: `-c <user> -a <ip[:port]> [--format deeplink|toml]`
- Автоконфиг клиента: `setup_wizard --mode non-interactive ...`
