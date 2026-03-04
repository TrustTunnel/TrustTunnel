package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestChecklistLifecycle(t *testing.T) {
	tmp := t.TempDir()
	app := &sidecarApp{
		nodeID:           "node-a",
		checklist:        filepath.Join(tmp, "checklist.json"),
		clientsPath:      filepath.Join(tmp, "clients.json"),
		aktDir:           filepath.Join(tmp, "akt"),
		executedCommands: map[string]bool{},
	}

	app.initChecklist()
	app.updateChecklistTask("register-node", "done")

	state := app.loadChecklist()
	if len(state.Checklist) == 0 {
		t.Fatal("checklist should be initialized")
	}

	found := false
	for _, task := range state.Checklist {
		if task.ID == "register-node" {
			found = true
			if task.Status != "done" {
				t.Fatalf("expected done, got %s", task.Status)
			}
		}
	}
	if !found {
		t.Fatal("register-node task not found")
	}
}

func TestApplyClientsStubAndCount(t *testing.T) {
	tmp := t.TempDir()
	app := &sidecarApp{
		clientsPath:      filepath.Join(tmp, "clients.json"),
		executedCommands: map[string]bool{},
	}
	app.applyClientsStub()
	if c := app.readClientsCount(); c != 1 {
		t.Fatalf("expected 1 client, got %d", c)
	}
}

func TestGenerateAktCreatesJsonAndHuman(t *testing.T) {
	tmp := t.TempDir()
	app := &sidecarApp{
		nodeID:           "node-z",
		checklist:        filepath.Join(tmp, "checklist.json"),
		aktDir:           filepath.Join(tmp, "akt"),
		executedCommands: map[string]bool{},
	}
	app.initChecklist()

	aktURL := app.generateAkt("cmd-1", "apply_configmap")
	if aktURL == "" {
		t.Fatal("aktURL should not be empty")
	}

	files, err := os.ReadDir(app.aktDir)
	if err != nil {
		t.Fatalf("expected akt dir to exist: %v", err)
	}
	if len(files) < 2 {
		t.Fatalf("expected json+txt artifacts, got %d files", len(files))
	}

	state := app.loadChecklist()
	if len(state.Akt) == 0 {
		t.Fatal("checklist should include akt payload")
	}

	var parsed map[string]interface{}
	if err := json.Unmarshal(state.Akt, &parsed); err != nil {
		t.Fatalf("akt payload is not valid json: %v", err)
	}
}
