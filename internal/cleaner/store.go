package cleaner

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/egoist/mygo/plugins/sqlite"
)

type Store struct{ db *sqlite.DB }

func OpenStore(ctx context.Context, directory string) (*Store, error) {
	if err := os.MkdirAll(directory, 0700); err != nil {
		return nil, err
	}
	db, err := sqlite.Open(ctx, filepath.Join(directory, "cleaner.db"), sqlite.OpenOptions{})
	if err != nil {
		return nil, err
	}
	for _, sql := range []string{
		`CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL)`,
		`CREATE TABLE IF NOT EXISTS candidates (id TEXT PRIMARY KEY, json TEXT NOT NULL)`,
		`CREATE TABLE IF NOT EXISTS ignored (id TEXT PRIMARY KEY)`,
		`CREATE TABLE IF NOT EXISTS actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, action TEXT NOT NULL, path TEXT NOT NULL, result TEXT NOT NULL)`,
	} {
		if _, err := db.Execute(ctx, sql); err != nil {
			db.Close()
			return nil, err
		}
	}
	return &Store{db: db}, nil
}

func (s *Store) Close() error { return s.db.Close() }

func (s *Store) LoadSettings(ctx context.Context, home string) (Settings, error) {
	rows, err := s.db.Query(ctx, `SELECT json FROM settings WHERE id=1`)
	if err != nil {
		return Settings{}, err
	}
	if len(rows.Values) == 0 {
		return DefaultSettings(home), nil
	}
	var settings Settings
	if err := json.Unmarshal([]byte(rows.Values[0][0].(string)), &settings); err != nil {
		return Settings{}, err
	}
	return settings, settings.Validate()
}

func (s *Store) SaveSettings(ctx context.Context, settings Settings) error {
	if err := settings.Validate(); err != nil {
		return err
	}
	data, err := json.Marshal(settings)
	if err != nil {
		return err
	}
	_, err = s.db.Execute(ctx, `INSERT INTO settings(id,json) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET json=excluded.json`, string(data))
	return err
}

func (s *Store) LoadCandidates(ctx context.Context) ([]Candidate, error) {
	rows, err := s.db.Query(ctx, `SELECT c.json, i.id FROM candidates c LEFT JOIN ignored i ON c.id=i.id ORDER BY c.id`)
	if err != nil {
		return nil, err
	}
	out := make([]Candidate, 0, len(rows.Values))
	for _, row := range rows.Values {
		var c Candidate
		if err := json.Unmarshal([]byte(row[0].(string)), &c); err != nil {
			return nil, err
		}
		c.Ignored = row[1] != nil
		out = append(out, c)
	}
	return out, nil
}

func (s *Store) ReplaceCandidates(ctx context.Context, candidates []Candidate) error {
	statements := []sqlite.Statement{{SQL: `DELETE FROM candidates`}}
	for _, c := range candidates {
		data, err := json.Marshal(c)
		if err != nil {
			return err
		}
		statements = append(statements, sqlite.Statement{SQL: `INSERT INTO candidates(id,json) VALUES(?,?)`, Args: []any{c.ID, string(data)}})
	}
	_, err := s.db.Transaction(ctx, statements)
	return err
}

func (s *Store) Ignore(ctx context.Context, ids []string) error {
	statements := make([]sqlite.Statement, 0, len(ids))
	for _, id := range ids {
		statements = append(statements, sqlite.Statement{SQL: `INSERT OR IGNORE INTO ignored(id) VALUES(?)`, Args: []any{id}})
	}
	_, err := s.db.Transaction(ctx, statements)
	return err
}

func (s *Store) Unignore(ctx context.Context, ids []string) error {
	statements := make([]sqlite.Statement, 0, len(ids))
	for _, id := range ids {
		statements = append(statements, sqlite.Statement{SQL: `DELETE FROM ignored WHERE id=?`, Args: []any{id}})
	}
	_, err := s.db.Transaction(ctx, statements)
	return err
}

func (s *Store) RemoveCandidates(ctx context.Context, ids []string) error {
	statements := make([]sqlite.Statement, 0, len(ids))
	for _, id := range ids {
		statements = append(statements, sqlite.Statement{SQL: `DELETE FROM candidates WHERE id=?`, Args: []any{id}})
	}
	_, err := s.db.Transaction(ctx, statements)
	return err
}

func (s *Store) RecordAction(ctx context.Context, at, action, path, result string) error {
	if len(result) > 2000 {
		result = result[:2000]
	}
	_, err := s.db.Execute(ctx, `INSERT INTO actions(at,action,path,result) VALUES(?,?,?,?)`, at, action, path, result)
	return err
}

func (s *Store) ActionCount(ctx context.Context) (int64, error) {
	rows, err := s.db.Query(ctx, `SELECT count(*) FROM actions`)
	if err != nil {
		return 0, err
	}
	if len(rows.Values) == 0 {
		return 0, fmt.Errorf("missing count")
	}
	return rows.Values[0][0].(int64), nil
}
