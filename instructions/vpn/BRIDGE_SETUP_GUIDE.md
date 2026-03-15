# Мостовая схема VPN: TrustTunnel → Российский VPS → VLESS+REALITY → Зарубежный VPS

Схема маршрутизации:

Клиент (телефон/ПК) → **TrustTunnel** (протокол на базе HTTPS/QUIC) → **Российский VPS (Мост)** → **SOCKS5 (Xray-клиент)** → **VLESS+REALITY** → **Зарубежный VPS (Цель)** → Интернет

Инструкция рассчитана на специалиста среднего уровня, знакомого с Linux, systemd и SSH.

---

## 1. Подготовка инфраструктуры

### 1.1. Выбор провайдеров

- **Зарубежный VPS (Цель)**: ЕС/США, публичный IPv4.
- **Российский VPS (Мост)**: РФ, публичный IPv4.

### 1.2. Операционная система

Рекомендуется **Ubuntu 22.04 LTS**. Все команды выполняются под `root`.

---

## 2. Настройка Зарубежного VPS (Цель): Xray (VLESS+REALITY)

1. **Установка Xray**:
   ```bash
   bash <(curl -L https://github.com/XTLS/Xray-install/raw/main/install-release.sh) install
   ```

2. **Параметры REALITY**:
   Сгенерируйте UUID (`cat /proc/sys/kernel/random/uuid`) и ключи (`xray x25519`). 
   Выберите victim-домен (например, `www.microsoft.com`).

3. **Конфигурация**: Настройте `/usr/local/etc/xray/config.json` на порт `443` с протоколом `vless` и безопасностью `reality`.

---

## 3. Настройка Российского VPS (Мост)

### 3.1. Установка TrustTunnel Endpoint

```bash
curl -fsSL https://raw.githubusercontent.com/TrustTunnel/TrustTunnel/refs/heads/master/scripts/install.sh | sh -s -
```
Бинарные файлы устанавливаются в `/opt/trusttunnel/`.

### 3.2. Инициализация через Setup Wizard

Для первой настройки выполните:
```bash
cd /opt/trusttunnel
sudo ./setup_wizard
```
Следуйте инструкциям мастера. По умолчанию TrustTunnel настраивается на порт **443**. 

### 3.3. Настройка форвардинга трафика

В созданном файле `vpn.toml` (в `/opt/trusttunnel/`) убедитесь, что трафик перенаправляется на локальный SOCKS5-порт Xray-клиента:

```toml
[forward_protocol.socks5]
address = "127.0.0.1:1080"
```

Для ознакомления со всеми параметрами конфигурационных файлов обратитесь к официальной документации:
[TrustTunnel Configuration Reference](https://github.com/TrustTunnel/TrustTunnel/blob/master/CONFIGURATION.md)

### 3.4. Xray-клиент на Российском VPS

Установите Xray и настройте входящий SOCKS5 на порту `1080`, который будет пересылать трафик на Зарубежный VPS через VLESS+REALITY.

---

## 4. Управление и SSL

### 4.1. Обновление сертификатов

Если вы используете Let's Encrypt через Certbot, для переключения на новый сертификат в TrustTunnel используйте `./setup_wizard` и выберите пункт `Existing Certificate`, указав пути к файлам в `/etc/letsencrypt/live/`.

Для автоматизации перезапуска добавьте `systemctl reload trusttunnel` (поддерживает SIGHUP для перечитывания конфигов) в хуки продления Certbot.

### 4.2. Запуск сервиса

```bash
systemctl enable --now trusttunnel
```

---

## 5. Проверка

1. Сгенерируйте клиентский конфиг:
   ```bash
   cd /opt/trusttunnel
   ./trusttunnel_endpoint vpn.toml hosts.toml -c <user> -a <domain_or_ip>
   ```
2. Подключитесь с клиента и проверьте внешний IP (должен быть IP Зарубежного VPS).
