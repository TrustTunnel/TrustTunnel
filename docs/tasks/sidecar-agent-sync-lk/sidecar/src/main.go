package main

import (
	"encoding/json"
	"fmt"
	"io/ioutil"
	"log"
	"net/http"
	"os"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

/*
Sidecar external checklist requirement:
1) Sidecar создаёт внешний checklist с pending (красный крестик по умолчанию).
2) По выполнении задач помечает done (зелёный) и создаёт актработа (human + json) в том же месте.
*/

type WSMessage struct {
	Kind      string                 `json:"kind"`
	CommandID string                 `json:"command_id,omitempty"`
	Type      string                 `json:"type,omitempty"`
	Payload   map[string]interface{} `json:"payload,omitempty"`
	Status    string                 `json:"status,omitempty"`
	AktURL    string                 `json:"akt_url,omitempty"`
	Details   map[string]interface{} `json:"details,omitempty"`
}

var (
	checklistPath    = "/tmp/checklist.json"
	nodeID           string
	lkWs             string
	token            string
	wsConn           *websocket.Conn
	mu               sync.Mutex
	executedCommands = map[string]bool{}
)

func main() {
	nodeID = os.Getenv("NODE_ID")
	token = os.Getenv("LK_TOKEN")
	lkWs = os.Getenv("LK_WS_ENDPOINT")

	if nodeID == "" || token == "" || lkWs == "" {
		log.Fatal("NODE_ID, LK_TOKEN and LK_WS_ENDPOINT env are required")
	}

	initChecklist()
	connectAndRun()
}

func initChecklist() {
	cl := map[string]interface{}{
		"checklist": []map[string]interface{}{
			{"id": "sync-configmap", "title": "Sync ConfigMap", "status": "pending"},
			{"id": "register-node", "title": "Register node in LK", "status": "pending"},
			{"id": "clients-sync", "title": "Clients sync", "status": "pending"},
		},
		"akt": nil,
	}
	b, _ := json.MarshalIndent(cl, "", "  ")
	_ = ioutil.WriteFile(checklistPath, b, 0644)
}

func connectAndRun() {
	url := fmt.Sprintf("%s?node_id=%s&token=%s", lkWs, nodeID, token)
	header := http.Header{}

	for {
		conn, _, err := websocket.DefaultDialer.Dial(url, header)
		if err != nil {
			log.Println("WS dial error:", err)
			time.Sleep(5 * time.Second)
			continue
		}
		wsConn = conn
		log.Println("WS connected")

		sendRegister()
		go heartbeatLoop()

		readLoop(conn)

		log.Println("WS disconnected, reconnecting...")
		time.Sleep(3 * time.Second)
	}
}

func sendRegister() {
	reg := WSMessage{
		Kind: "register",
		Payload: map[string]interface{}{
			"node_id":     nodeID,
			"fingerprint": "fp-example",
			"ingress_ip":  "127.0.0.1",
			"max_clients": 250,
		},
	}
	_ = wsConn.WriteJSON(reg)
	updateChecklistTask("register-node", "done")
}

func heartbeatLoop() {
	ticker := time.NewTicker(30 * time.Second)
	for range ticker.C {
		hb := WSMessage{
			Kind: "heartbeat",
			Payload: map[string]interface{}{
				"node_id":       nodeID,
				"clients_count": 0,
				"status":        "online",
			},
		}
		_ = wsConn.WriteJSON(hb)
	}
}

func readLoop(conn *websocket.Conn) {
	for {
		var msg WSMessage
		if err := conn.ReadJSON(&msg); err != nil {
			log.Println("Read error:", err)
			_ = conn.Close()
			return
		}
		switch msg.Kind {
		case "command":
			go handleCommand(msg)
		case "ping":
			_ = conn.WriteJSON(WSMessage{Kind: "pong"})
		default:
			log.Println("Unknown message kind:", msg.Kind)
		}
	}
}

func handleCommand(msg WSMessage) {
	mu.Lock()
	if executedCommands[msg.CommandID] {
		mu.Unlock()
		_ = wsConn.WriteJSON(WSMessage{Kind: "result", CommandID: msg.CommandID, Status: "done"})
		return
	}
	executedCommands[msg.CommandID] = true
	mu.Unlock()

	_ = wsConn.WriteJSON(WSMessage{Kind: "ack", CommandID: msg.CommandID, Status: "accepted"})

	// Simulate work; replace with real logic (configmap sync, client updates, etc)
	time.Sleep(1 * time.Second)
	updateChecklistTask("clients-sync", "done")

	aktPath := generateAkt(msg.CommandID, []map[string]string{
		{"id": "clients-sync", "time": time.Now().UTC().Format(time.RFC3339), "notes": "ok"},
	})

	_ = wsConn.WriteJSON(WSMessage{
		Kind:      "result",
		CommandID: msg.CommandID,
		Status:    "done",
		AktURL:    "file://" + aktPath,
		Details:   map[string]interface{}{"note": "command executed"},
	})
}

func updateChecklistTask(taskID, status string) {
	mu.Lock()
	defer mu.Unlock()

	b, err := ioutil.ReadFile(checklistPath)
	if err != nil {
		return
	}
	var cl map[string]interface{}
	_ = json.Unmarshal(b, &cl)

	arr, ok := cl["checklist"].([]interface{})
	if !ok {
		return
	}
	for i := range arr {
		item, ok := arr[i].(map[string]interface{})
		if ok && item["id"] == taskID {
			item["status"] = status
		}
	}
	nb, _ := json.MarshalIndent(cl, "", "  ")
	_ = ioutil.WriteFile(checklistPath, nb, 0644)
}

func generateAkt(commandID string, tasks []map[string]string) string {
	akt := map[string]interface{}{
		"generated_at":    time.Now().UTC().Format(time.RFC3339),
		"tasks_completed": tasks,
		"summary":         fmt.Sprintf("Command %s executed", commandID),
	}
	b, _ := json.MarshalIndent(akt, "", "  ")
	path := "/tmp/akt-" + commandID + ".json"
	_ = ioutil.WriteFile(path, b, 0644)
	return path
}
