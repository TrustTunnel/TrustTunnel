#!/bin/bash

# --- НАСТРОЙКИ ---
TOKEN=""
MY_CHAT_ID=""
IP_SERVER=""
TIME_DIR="/tmp/vpn_times"
METRICS_URL="http://127.0.0.1:1987/clients"
# -----------------

LAST_UPDATE_ID=0

format_time() {
    local sec=$1
    printf '%dh %dm %ds' $((sec/3600)) $((sec%3600/60)) $((sec%60))
}

format_traffic_clients() {
    local json="$1"
    local count
    count=$(echo "$json" | jq 'length')
    if [[ "$count" -eq 0 ]]; then
        echo "ℹ️ Нет данных о клиентах."
        return
    fi

    local TEXT="📶 *Трафик по клиентам VPN:*"
    while IFS= read -r block; do
        [[ -n "$block" ]] && TEXT+="%0A%0A$block"
    done < <(echo "$json" | jq -r '.[] |
        (if .quota_exceeded then "⛔" else "✅" end) as $icon |
        (if .limit != null and .limit > 0
            then ((.total * 100 / .limit) | floor | tostring) + "%"
            else "—" end) as $pct |
        (if .outbound >= 1073741824 then ((.outbound / 1073741824 * 100 | round) / 100 | tostring) + " GiB"
            elif .outbound >= 1048576 then ((.outbound / 1048576 * 10 | round) / 10 | tostring) + " MiB"
            elif .outbound >= 1024 then ((.outbound / 1024 * 10 | round) / 10 | tostring) + " KiB"
            else (.outbound | tostring) + " B" end) as $down |
        (if .inbound >= 1073741824 then ((.inbound / 1073741824 * 100 | round) / 100 | tostring) + " GiB"
            elif .inbound >= 1048576 then ((.inbound / 1048576 * 10 | round) / 10 | tostring) + " MiB"
            elif .inbound >= 1024 then ((.inbound / 1024 * 10 | round) / 10 | tostring) + " KiB"
            else (.inbound | tostring) + " B" end) as $up |
        (if .total >= 1073741824 then ((.total / 1073741824 * 100 | round) / 100 | tostring) + " GiB"
            elif .total >= 1048576 then ((.total / 1048576 * 10 | round) / 10 | tostring) + " MiB"
            elif .total >= 1024 then ((.total / 1024 * 10 | round) / 10 | tostring) + " KiB"
            else (.total | tostring) + " B" end) as $sum |
        (if .limit != null
            then (if .limit >= 1073741824 then ((.limit / 1073741824 * 100 | round) / 100 | tostring) + " GiB"
                  elif .limit >= 1048576 then ((.limit / 1048576 * 10 | round) / 10 | tostring) + " MiB"
                  else (.limit | tostring) + " B" end)
            else "∞" end) as $lim |
        $icon + " *" + .username + "* — " + (.sessions | tostring) + " сесс." +
        (if .ip != null then "%0A   🌐 `" + .ip + "`" else "" end) +
        "%0A   ⬇️ " + $down + "  ⬆️ " + $up +
        "%0A   Σ *" + $sum + "* / " + $lim +
        (if .limit != null then " (" + $pct + ")" else "" end)
    ')

    echo "$TEXT"
}

while true; do
    RESPONSE=$(curl -s "https://api.telegram.org/bot$TOKEN/getUpdates?offset=$((LAST_UPDATE_ID + 1))&timeout=30")
    UPDATE_ID=$(echo "$RESPONSE" | jq -r '.result[0].update_id // empty')

    if [[ -n "$UPDATE_ID" ]]; then
        LAST_UPDATE_ID=$UPDATE_ID
        MESSAGE=$(echo "$RESPONSE" | jq -r '.result[0].message.text // empty')
        CHAT_ID=$(echo "$RESPONSE" | jq -r '.result[0].message.chat.id // empty')

        if [[ "$CHAT_ID" == "$MY_CHAT_ID" ]]; then
            TEXT=""

            # --- КОМАНДА /status ---
            if [[ "$MESSAGE" == "/status" ]]; then
                PID=$(pgrep -x trusttunnel_end | head -n 1)
                if [[ -z "$PID" ]]; then
                    TEXT="⚠️ Процесс VPN не найден."
                else
                    IPS=$(lsof -p $PID -i -n 2>/dev/null | grep "$IP_SERVER:https" | awk '{print $9}' | cut -d'>' -f2 | cut -d':' -f1 | sort | uniq)
                    if [[ -z "$IPS" ]]; then
                        TEXT="ℹ️ Активных соединений нет."
                    else
                        TEXT="📊 *Текущие подключения:*"
                        NOW=$(date +%s)
                        while read -r ip; do
                            [ -z "$ip" ] && continue
                            START=$(cat "$TIME_DIR/$ip.start" 2>/dev/null)
                            DUR=$([ -n "$START" ] && format_time $((NOW - START)) || echo "только что")
                            IP_INFO=$(curl -s "http://ip-api.com/json/$ip?fields=status,country,city,isp,org,as,mobile,proxy,hosting")
                            GEO_STATUS=$(echo "$IP_INFO" | jq -r '.status // empty')
                            IP_TAG="#ip_${ip//./_}"
                            if [[ "$GEO_STATUS" == "success" ]]; then
                                COUNTRY=$(echo "$IP_INFO" | jq -r '.country'); CITY=$(echo "$IP_INFO" | jq -r '.city')
                                ISP=$(echo "$IP_INFO" | jq -r '.isp'); ORG=$(echo "$IP_INFO" | jq -r '.org // empty')
                                AS_INFO=$(echo "$IP_INFO" | jq -r '.as'); MOB=$(echo "$IP_INFO" | jq -r '.mobile')
                                PROXY=$(echo "$IP_INFO" | jq -r '.proxy'); HOSTING=$(echo "$IP_INFO" | jq -r '.hosting')
                                TAGS=""
                                [[ "$MOB" == "true" ]] && TAGS+=" 📱"
                                [[ "$PROXY" == "true" ]] && TAGS+=" 🛡️"
                                [[ "$HOSTING" == "true" ]] && TAGS+=" ☁️"
                                TEXT+="%0A%0A🌐 \`$ip\` — *$DUR*$TAGS%0A🆔 $IP_TAG%0A📍 $COUNTRY, $CITY"
                                [[ -n "$ORG" && "$ORG" != "null" && "$ORG" != "$ISP" ]] && TEXT+="%0A🏢 $ORG"
                                TEXT+="%0A📡 $ISP ($AS_INFO)"
                            else
                                TEXT+="%0A%0A🌐 \`$ip\` — *$DUR*%0A🆔 $IP_TAG%0A⚠️ GeoIP недоступен."
                            fi
                        done <<< "$IPS"
                    fi
                fi
                curl -s -X POST "https://api.telegram.org/bot$TOKEN/sendMessage" -d "chat_id=$CHAT_ID&text=$TEXT&parse_mode=Markdown&disable_web_page_preview=true" > /dev/null

            # --- КОМАНДА /traffic ---
            elif [[ "$MESSAGE" == "/traffic" || "$MESSAGE" =~ ^/traffic[[:space:]] ]]; then
                FILTER_USER=""
                if [[ "$MESSAGE" =~ ^/traffic[[:space:]]+(.+)$ ]]; then
                    FILTER_USER="${BASH_REMATCH[1]}"
                fi

                CLIENTS_JSON=$(curl -s --connect-timeout 3 "$METRICS_URL")
                if [[ -z "$CLIENTS_JSON" || "$CLIENTS_JSON" == "[]" ]]; then
                    TEXT="⚠️ Нет данных.%0A%0AПроверьте:%0A• \`[metrics]\` в vpn.toml%0A• endpoint запущен%0A• собрана версия с /clients"
                elif ! echo "$CLIENTS_JSON" | jq -e 'type == "array"' >/dev/null 2>&1; then
                    TEXT="⚠️ /clients вернул неожиданный ответ.%0AУстановлена ли новая версия TrustTunnel?"
                else
                    if [[ -n "$FILTER_USER" ]]; then
                        CLIENTS_JSON=$(echo "$CLIENTS_JSON" | jq --arg u "$FILTER_USER" '[.[] | select(.username == $u)]')
                        if [[ $(echo "$CLIENTS_JSON" | jq 'length') -eq 0 ]]; then
                            TEXT="⚠️ Пользователь \`$FILTER_USER\` не найден."
                        else
                            TEXT=$(format_traffic_clients "$CLIENTS_JSON")
                        fi
                    else
                        TEXT=$(format_traffic_clients "$CLIENTS_JSON")
                    fi
                fi
                curl -s -X POST "https://api.telegram.org/bot$TOKEN/sendMessage" -d "chat_id=$CHAT_ID&text=$TEXT&parse_mode=Markdown" > /dev/null

            # --- КОМАНДА /top ---
            elif [[ "$MESSAGE" == "/top" ]]; then
                TT_VER=$(/opt/trusttunnel/trusttunnel_endpoint --version 2>/dev/null | xargs)
                [ -z "$TT_VER" ] && TT_VER="неизвестно"
                LATEST_VER=$(curl -s https://api.github.com/repos/TrustTunnel/TrustTunnel/releases/latest | jq -r '.tag_name // empty' | sed 's/^v//')
                VER_DISPLAY="*$TT_VER*"
                [[ -n "$LATEST_VER" && "$TT_VER" != "$LATEST_VER" && "$TT_VER" != "неизвестно" ]] && VER_DISPLAY="*$TT_VER* ⚠️ (Доступна: $LATEST_VER)"

                CPU=$(top -bn1 | grep "Cpu(s)" | awk '{print $2 + $4}')
                RAM=$(free -m | awk '/Mem:/ { printf("%.2f%% (%d/%d MB)", $3/$2*100, $3, $2) }')
                DISK=$(df -h / | awk '/\// {print $5}' | tail -n 1)
                UPTIME=$(uptime -p)
                CERT_DATA=$(certbot certificates 2>/dev/null | grep -E "Certificate Name:|Expiry Date:" | sed 's/^[[:space:]]*//')

                TEXT="🖥 **Статус сервера:**%0A🛡 TrustTunnel: $VER_DISPLAY%0A🔥 CPU: *$CPU%*%0A📟 RAM: *$RAM*%0A💾 Диск: *$DISK*%0A⏱ Uptime: *$UPTIME*%0A%0A🔐 **Сертификаты:**%0A"
                if [[ -z "$CERT_DATA" ]]; then TEXT+="⚠️ Данные не найдены"; else
                    FORMATTED_CERTS=$(echo "$CERT_DATA" | sed ':a;N;$!ba;s/\n/%0A/g')
                    TEXT+="\`\`\`%0A$FORMATTED_CERTS%0A\`\`\`"; fi
                curl -s -X POST "https://api.telegram.org/bot$TOKEN/sendMessage" -d "chat_id=$CHAT_ID&text=$TEXT&parse_mode=Markdown" > /dev/null

            # --- КОМАНДА /net ---
            elif [[ "$MESSAGE" == "/net" ]]; then
                R1=$(awk '{if(NR>2) s+=$2} END {print s}' /proc/net/dev); T1=$(awk '{if(NR>2) s+=$10} END {print s}' /proc/net/dev)
                sleep 1
                R2=$(awk '{if(NR>2) s+=$2} END {print s}' /proc/net/dev); T2=$(awk '{if(NR>2) s+=$10} END {print s}' /proc/net/dev)
                RXKB=$(( (R2 - R1) / 1024 )); TXKB=$(( (T2 - T1) / 1024 ))
                TRAFFIC=$(awk '{if(NR>2) {r+=$2; t+=$10}} END {printf "%.2f|%.2f|%.2f", r/1073741824, t/1073741824, (r+t)/1073741824}' /proc/net/dev)
                RX_TOTAL=$(echo "$TRAFFIC" | cut -d'|' -f1); TX_TOTAL=$(echo "$TRAFFIC" | cut -d'|' -f2); SUM_TOTAL=$(echo "$TRAFFIC" | cut -d'|' -f3)

                TEXT="📊 **Сеть:**%0AСкорость: ⬇️ $RXKB KB/s | ⬆️ $TXKB KB/s%0AТрафик: ⬇️ $RX_TOTAL GB | ⬆️ $TX_TOTAL GB%0AВсего: *$SUM_TOTAL GB*"
                curl -s -X POST "https://api.telegram.org/bot$TOKEN/sendMessage" -d "chat_id=$CHAT_ID&text=$TEXT&parse_mode=Markdown" > /dev/null

            # --- КОМАНДА /update ---
            elif [[ "$MESSAGE" == "/update" ]]; then
                CURRENT_VER=$(/opt/trusttunnel/trusttunnel_endpoint --version 2>/dev/null | xargs)
                LATEST_TAG=$(curl -s https://api.github.com/repos/TrustTunnel/TrustTunnel/releases/latest | jq -r '.tag_name // empty')
                LATEST_VER=$(echo "$LATEST_TAG" | sed 's/^v//')
                LOG_FILE="/tmp/tt_update.log"
                TIMESTAMP=$(date "+[%Y-%m-%d %H:%M:%S]")

                if [[ -z "$LATEST_TAG" ]]; then
                    TEXT="❌ Не удалось проверить обновления на GitHub."
                elif [[ "$CURRENT_VER" == "$LATEST_VER" ]]; then
                    TEXT="✅ У вас уже последняя версия (*$CURRENT_VER*)."
                else
                    curl -s -X POST "https://api.telegram.org/bot$TOKEN/sendMessage" -d "chat_id=$CHAT_ID&text=🚀 Обновление *$CURRENT_VER* -> *$LATEST_VER*...&parse_mode=Markdown" > /dev/null

                    echo "$TIMESTAMP --- Начало обновления с $CURRENT_VER до $LATEST_VER ---" >> "$LOG_FILE"

                    sudo systemctl stop trusttunnel
                    ARCH_NAME="trusttunnel-${LATEST_TAG}-linux-x86_64"
                    ARCHIVE="/tmp/${ARCH_NAME}.tar.gz"

                    if curl -L -o "$ARCHIVE" "https://github.com/TrustTunnel/TrustTunnel/releases/download/${LATEST_TAG}/${ARCH_NAME}.tar.gz" >> "$LOG_FILE" 2>&1; then
                        if tar -xzf "$ARCHIVE" -C /opt/trusttunnel --strip-components=1 "${ARCH_NAME}/trusttunnel_endpoint" >> "$LOG_FILE" 2>&1; then
                            chmod +x /opt/trusttunnel/trusttunnel_endpoint
                            sudo systemctl start trusttunnel
                            TEXT="✅ *Обновление до $LATEST_VER завершено!*"
                            echo "$(date "+[%Y-%m-%d %H:%M:%S]") Успешно обновлено до $LATEST_VER" >> "$LOG_FILE"
                        else
                            sudo systemctl start trusttunnel
                            TEXT="❌ Ошибка при распаковке. Откат к старой версии."
                            echo "$(date "+[%Y-%m-%d %H:%M:%S]") ОШИБКА: Распаковка не удалась" >> "$LOG_FILE"
                        fi
                    else
                        sudo systemctl start trusttunnel
                        TEXT="❌ Ошибка при скачивании файла."
                        echo "$(date "+[%Y-%m-%d %H:%M:%S]") ОШИБКА: Скачивание не удалось" >> "$LOG_FILE"
                    fi

                    rm -f "$ARCHIVE"
                    if [[ -f "$LOG_FILE" ]]; then
                        echo "$(tail -n 50 "$LOG_FILE")" > "$LOG_FILE"
                    fi
                fi
                curl -s -X POST "https://api.telegram.org/bot$TOKEN/sendMessage" -d "chat_id=$CHAT_ID&text=$TEXT&parse_mode=Markdown" > /dev/null
            fi
        fi
    fi
done
