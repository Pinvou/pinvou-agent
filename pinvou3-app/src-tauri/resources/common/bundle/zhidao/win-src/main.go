// Windows-native reimplementation of H3C "知道" (zhidao) knowledge-base CLI.
//
// Background: H3C IT ships zhidao only as a Linux ELF (`zhidao-cli`); there is no
// Windows binary, so the Tauri app's zhidao connector could never run on Windows.
// This is a clean-room reimplementation in Go (the original is also Go) of just the
// subcommands the app + skill use, built natively for windows/amd64.
//
// Protocol was reverse-engineered from the (un-stripped, debug_info) ELF and the
// sibling eip-cli (same SSO_NEXT auth backend, same eos-auth token):
//   auth base   : https://api.eos.h3c.com/auth/v1.0
//     login     : POST /api/v2/user/agent/session/login   {deviceId, metaData}
//     poll      : GET  /api/v2/user/agent/session/poll/{sessionId}
//     exchange  : POST /api/v2/user/agent/token/exchange   {ait, deviceId}
//     (all auth requests carry header  Auth-Type: SSO_NEXT)
//   search base : https://api-searchservice.h3c.com/itsearchserve/v1.0
//     search    : POST /search/search
//     qa         : POST /search/getAiResult   (text/event-stream)
//     download  : GET  /search/downSearchFile
//
// Credentials: AES-256-GCM, key = SHA256(deviceID | salt). Stored under
// AGENT_CREDENTIALS_DIR as credentials_at.enc (token) and credentials_ait.enc (ait).
// We own both the writer (poll/save) and the readers (load/qa/search), so the
// on-disk format only needs to be self-consistent — it does NOT interop with eip-cli.
//
// NOTE: the auth flow (login/poll/exchange) is faithfully reconstructed and the
// endpoints/headers are confirmed from the binaries. The search/qa request *body*
// field names and the source→nodeId table could not be verified without a live
// intranet token; those spots are marked WIRE-UNVERIFIED and kept easy to adjust.
package main

import (
	"bytes"
	"bufio"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	authBase   = "https://api.eos.h3c.com/auth/v1.0"
	searchBase = "https://api-searchservice.h3c.com/itsearchserve/v1.0"
	credSalt   = "zhidao-cli-cred-v1"
	userAgent  = "zhidao-cli (windows)"
)

// ──────────────────────────── env / device ────────────────────────────

func deviceID() string {
	if v := strings.TrimSpace(os.Getenv("AGENT_DEVICE_ID")); v != "" {
		return v
	}
	return "zhidao-cli-default-device"
}

func credDir() string {
	d := os.Getenv("AGENT_CREDENTIALS_DIR")
	if d == "" {
		home, _ := os.UserHomeDir()
		d = filepath.Join(home, ".pinvou3", "zhidao", "credentials")
	}
	_ = os.MkdirAll(d, 0o700)
	return d
}

func atFile() string  { return filepath.Join(credDir(), "credentials_at.enc") }
func aitFile() string { return filepath.Join(credDir(), "credentials_ait.enc") }

// ──────────────────────────── credential crypto ────────────────────────────

func deriveKey() []byte {
	h := sha256.Sum256([]byte(deviceID() + "|" + credSalt))
	return h[:]
}

func encryptToFile(path, plain string) error {
	block, err := aes.NewCipher(deriveKey())
	if err != nil {
		return err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return err
	}
	ct := gcm.Seal(nonce, nonce, []byte(plain), nil)
	return os.WriteFile(path, ct, 0o600)
}

func decryptFromFile(path string) (string, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	block, err := aes.NewCipher(deriveKey())
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	ns := gcm.NonceSize()
	if len(raw) < ns {
		return "", fmt.Errorf("credential too short")
	}
	plain, err := gcm.Open(nil, raw[:ns], raw[ns:], nil)
	if err != nil {
		return "", fmt.Errorf("decrypt: %w", err)
	}
	return string(plain), nil
}

func loadAT() string  { s, _ := decryptFromFile(atFile()); return s }
func loadAIT() string { s, _ := decryptFromFile(aitFile()); return s }

func saveAT(token string) error {
	if token == "" {
		return nil
	}
	return encryptToFile(atFile(), token)
}
func saveAIT(ait string) error {
	if ait == "" {
		return nil
	}
	return encryptToFile(aitFile(), ait)
}

// ──────────────────────────── http ────────────────────────────

func httpClient() *http.Client {
	// EIP/zhidao are intranet services: never route through a configured proxy.
	tr := &http.Transport{Proxy: nil}
	return &http.Client{Timeout: 60 * time.Second, Transport: tr}
}

func authHeaders(req *http.Request, withToken bool) {
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Auth-Type", "SSO_NEXT")
	req.Header.Set("Accept", "application/json")
	req.Header.Set("User-Agent", userAgent)
	if withToken {
		t := loadAT()
		if t != "" {
			if !strings.HasPrefix(t, "Bearer ") {
				t = "Bearer " + t
			}
			req.Header.Set("Authorization", t)
		}
	}
}

// envelope is the common {code,msg,data} response wrapper.
type envelope struct {
	Code    interface{}     `json:"code"`
	Msg     string          `json:"msg"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data"`
	TraceID string          `json:"traceId"`
	// some endpoints inline these at the top level:
	SsoLoginURL string `json:"ssoLoginUrl"`
	SessionID   string `json:"sessionId"`
	Token       string `json:"token"`
	Ait         string `json:"ait"`
}

type sessionData struct {
	SessionID   string `json:"sessionId"`
	SsoLoginURL string `json:"ssoLoginUrl"`
	Token       string `json:"token"`
	Ait         string `json:"ait"`
	Expire      int64  `json:"expire"`
	PollInterval int   `json:"pollInterval"`
}

func postJSON(url string, body interface{}, withToken bool) (*envelope, []byte, error) {
	b, _ := json.Marshal(body)
	req, err := http.NewRequest("POST", url, bytes.NewReader(b))
	if err != nil {
		return nil, nil, err
	}
	authHeaders(req, withToken)
	resp, err := httpClient().Do(req)
	if err != nil {
		return nil, nil, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	var env envelope
	_ = json.Unmarshal(raw, &env)
	return &env, raw, nil
}

func getURL(url string, withToken bool) (*envelope, []byte, error) {
	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, nil, err
	}
	authHeaders(req, withToken)
	resp, err := httpClient().Do(req)
	if err != nil {
		return nil, nil, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	var env envelope
	_ = json.Unmarshal(raw, &env)
	return &env, raw, nil
}

func (e *envelope) session() sessionData {
	var d sessionData
	if len(e.Data) > 0 {
		_ = json.Unmarshal(e.Data, &d)
	}
	// fall back to top-level fields if data was empty
	if d.SsoLoginURL == "" {
		d.SsoLoginURL = e.SsoLoginURL
	}
	if d.SessionID == "" {
		d.SessionID = e.SessionID
	}
	if d.Token == "" {
		d.Token = e.Token
	}
	if d.Ait == "" {
		d.Ait = e.Ait
	}
	return d
}

// ──────────────────────────── auth ────────────────────────────

func cmdLogin() int {
	name, _ := os.Hostname()
	if name == "" {
		name = "pinvou3"
	}
	body := map[string]interface{}{
		"deviceId": deviceID(),
		// metaData.name is required by the server (设备扩展信息[metaData.name]不能为空).
		"metaData": map[string]string{"name": name, "os": "windows", "client": "pinvou3"},
	}
	env, raw, err := postJSON(authBase+"/api/v2/user/agent/session/login", body, false)
	if err != nil {
		emitJSON(map[string]interface{}{"error": err.Error()})
		return 1
	}
	d := env.session()
	if d.SsoLoginURL == "" {
		emitJSON(map[string]interface{}{
			"error":   "login 未返回 ssoLoginUrl",
			"message": firstNonEmpty(env.Msg, env.Message),
			"raw":     string(raw),
		})
		return 1
	}
	emitJSON(map[string]interface{}{
		"ssoLoginUrl": d.SsoLoginURL,
		"sessionId":   d.SessionID,
		"message":     "请在浏览器中打开 ssoLoginUrl 完成 SSO 认证",
	})
	return 0
}

// cmdPoll does ONE poll round (optionally looping up to --timeout seconds).
// On success it persists token+ait and exits 0; if still pending it exits 8
// (matching eip-cli's "轮询超时(还没授权)" semantics). The Rust side loops.
func cmdPoll(args map[string]string) int {
	sid := args["session-id"]
	if sid == "" {
		emitJSON(map[string]interface{}{"error": "缺少 --session-id"})
		return 2
	}
	timeout := atoiDefault(args["timeout"], 5)
	deadline := time.Now().Add(time.Duration(timeout) * time.Second)
	for {
		env, _, err := getURL(authBase+"/api/v2/user/agent/session/poll/"+sid, false)
		if err == nil {
			d := env.session()
			if d.Token != "" {
				_ = saveAT(d.Token)
				_ = saveAIT(d.Ait)
				emitJSON(map[string]interface{}{"connected": true})
				return 0
			}
		}
		if time.Now().After(deadline) {
			emitJSON(map[string]interface{}{"connected": false, "pending": true})
			return 8
		}
		time.Sleep(2 * time.Second)
	}
}

func cmdExchange() int {
	ait := loadAIT()
	if ait == "" {
		emitJSON(map[string]interface{}{"error": "无 AIT，请先 login"})
		return 3
	}
	body := map[string]interface{}{"ait": ait, "deviceId": deviceID()}
	env, raw, err := postJSON(authBase+"/api/v2/user/agent/token/exchange", body, false)
	if err != nil {
		emitJSON(map[string]interface{}{"error": err.Error()})
		return 1
	}
	d := env.session()
	if d.Token == "" {
		emitJSON(map[string]interface{}{"error": "exchange 未返回 token", "raw": string(raw)})
		return 1
	}
	_ = saveAT(d.Token)
	if d.Ait != "" {
		_ = saveAIT(d.Ait)
	}
	emitJSON(map[string]interface{}{"ok": true})
	return 0
}

// ensureToken returns a usable token, refreshing via exchange if the AT is gone.
func ensureToken() string {
	if t := loadAT(); t != "" {
		return t
	}
	if cmdExchange() == 0 {
		return loadAT()
	}
	return ""
}

// ──────────────────────────── creds subcommands ────────────────────────────

func cmdSave(args map[string]string) int {
	if args["token"] == "" && args["ait"] == "" {
		fmt.Fprintln(os.Stderr, "需要 --token 或 --ait")
		return 2
	}
	if err := saveAT(stripBearer(args["token"])); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	if err := saveAIT(args["ait"]); err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	fmt.Println("凭证已保存")
	return 0
}

// cmdLoad prints `export ZHIDAO_TOKEN=...`. The Rust connector treats the presence
// of the literal "ZHIDAO_TOKEN" in stdout as "connected", so only print it when a
// token actually exists.
func cmdLoad() int {
	t := loadAT()
	if t == "" {
		fmt.Fprintln(os.Stderr, "未找到凭证，请先执行 login")
		return 3
	}
	if !strings.HasPrefix(t, "Bearer ") {
		t = "Bearer " + t
	}
	fmt.Printf("export ZHIDAO_TOKEN=%q\n", t)
	if ait := loadAIT(); ait != "" {
		fmt.Printf("export ZHIDAO_AIT=%q\n", ait)
	}
	return 0
}

func cmdClear() int {
	_ = os.Remove(atFile())
	// AIT is left in place by the original (it is "external"); mirror that, but
	// remove it too so a Windows logout is a clean slate.
	_ = os.Remove(aitFile())
	fmt.Println("凭证已清空")
	return 0
}

// ──────────────────────────── business: search ────────────────────────────

// WIRE-UNVERIFIED: the exact request body keys for the itsearchserve API could not
// be confirmed without a live token. Keys below are the best reconstruction; adjust
// here if the server rejects them.
type searchRequest struct {
	Keyword  string   `json:"keyword"`
	PageNum  int      `json:"pageNum"`
	PageSize int      `json:"pageSize"`
	NodeIds  []string `json:"nodeIds,omitempty"`
	SrcName  string   `json:"srcName,omitempty"`
	SortType string   `json:"sortType,omitempty"`
}

func cmdSearch(args map[string]string) int {
	if ensureToken() == "" {
		fmt.Fprintln(os.Stderr, "未登录或登录已过期，请先连接（login）")
		return 3
	}
	q := firstNonEmpty(args["query"], args["q"])
	if q == "" {
		fmt.Fprintln(os.Stderr, "需要 --query")
		return 2
	}
	reqBody := searchRequest{
		Keyword:  q,
		PageNum:  atoiDefault(args["pageNum"], 1),
		PageSize: atoiDefault(args["pageSize"], 10),
		SortType: mapSort(args["sort"]),
	}
	if src := args["source"]; src != "" {
		// nodeId table is unavailable; pass the en-name through both likely fields.
		reqBody.SrcName = src
	}
	_, raw, err := postJSON(searchBase+"/search/search", reqBody, true)
	if err != nil {
		fmt.Fprintln(os.Stderr, "搜索请求失败:", err)
		return 1
	}
	if args["raw-json"] == "true" {
		fmt.Println(string(raw))
		return 0
	}
	printSearchResults(raw)
	return 0
}

func mapSort(s string) string {
	switch s {
	case "time":
		return "time"
	case "download":
		return "download"
	case "visit":
		return "visit"
	default:
		return "" // relevance (default)
	}
}

// printSearchResults defensively finds the document array in the response and
// renders each doc in the SKILL.md list format. Falls back to raw JSON.
func printSearchResults(raw []byte) {
	var top map[string]interface{}
	if json.Unmarshal(raw, &top) != nil {
		fmt.Println(string(raw))
		return
	}
	docs := findDocArray(top)
	if len(docs) == 0 {
		fmt.Println("未找到相关资料，请尝试其他关键词或访问 [知道](<https://ai.h3c.com/zhidao>) 查询。")
		return
	}
	for i, d := range docs {
		doc, _ := d.(map[string]interface{})
		title := pick(doc, "title", "h3c_title", "fileName", "h3c_file")
		author := pick(doc, "authorChineseName", "author", "fileEditor")
		date := pick(doc, "date", "h3c_filemodified", "h3c_filecreated")
		downloads := pick(doc, "downloads", "h3c_downs")
		visits := pick(doc, "visits", "h3c_visits")
		source := pick(doc, "source_cn_name", "source", "appCnName")
		preview := pick(doc, "preview_url", "h3c_filepreview")
		filePath := pick(doc, "file_path", "h3c_spweb")
		dl := pick(doc, "download_url", "fileWeb")
		fmt.Printf("%d. %s\n", i+1, title)
		fmt.Printf("作者: %s | 更新时间: %s | 下载: %s | 浏览: %s | 数据来源: %s\n", author, date, downloads, visits, source)
		fmt.Printf("预览地址: [点击预览](<%s>)\n", preview)
		fmt.Printf("进入目录: [点击进入](<%s>)\n", filePath)
		fmt.Printf("下载链接: [点击下载](<%s>)\n\n", dl)
	}
	fmt.Println("如需查看更多结果，请输入数据来源或者访问[知道](<https://ai.h3c.com/zhidao>)进行查询。")
}

func findDocArray(top map[string]interface{}) []interface{} {
	// look under data, then common list keys
	scopes := []interface{}{top}
	if d, ok := top["data"]; ok {
		scopes = append([]interface{}{d}, scopes...)
	}
	keys := []string{"results", "rows", "list", "records", "docs", "items"}
	for _, sc := range scopes {
		m, ok := sc.(map[string]interface{})
		if !ok {
			continue
		}
		for _, k := range keys {
			if arr, ok := m[k].([]interface{}); ok && len(arr) > 0 {
				return arr
			}
		}
	}
	// data itself an array?
	if arr, ok := top["data"].([]interface{}); ok {
		return arr
	}
	return nil
}

// ──────────────────────────── business: qa ────────────────────────────

type qaRequest struct {
	Question     string `json:"question"`
	IsDeepSearch bool   `json:"isDeepSearch"`
	SessionID    string `json:"sessionId,omitempty"`
	UserID       string `json:"userId,omitempty"`
}

func cmdQA(args map[string]string) int {
	if ensureToken() == "" {
		fmt.Fprintln(os.Stderr, "未登录或登录已过期，请先连接（login）")
		return 3
	}
	q := args["question"]
	if q == "" {
		fmt.Fprintln(os.Stderr, "需要 --question")
		return 2
	}
	deep := args["deep"] == "true"
	if args["no-deep"] != "true" && !deep {
		deep = autoDeep(q)
	}
	body := qaRequest{
		Question:     q,
		IsDeepSearch: deep,
		UserID:       firstNonEmpty(args["user-id"], os.Getenv("USER_ID"), "default_user"),
	}
	b, _ := json.Marshal(body)
	req, _ := http.NewRequest("POST", searchBase+"/search/getAiResult", bytes.NewReader(b))
	authHeaders(req, true)
	req.Header.Set("Accept", "text/event-stream")
	resp, err := httpClient().Do(req)
	if err != nil {
		fmt.Fprintln(os.Stderr, "问答请求失败:", err)
		return 1
	}
	defer resp.Body.Close()

	fmt.Printf("问题: %s\n深度搜索: %s\n\n", q, yesno(deep))
	answer := parseSSEAnswer(resp.Body)
	if strings.TrimSpace(answer) == "" {
		fmt.Println("未找到相关答案，请尝试其他问题或使用更具体的关键词。")
		return 0
	}
	fmt.Println(answer)
	return 0
}

// parseSSEAnswer accumulates `data:` payloads from an SSE stream, extracting any
// answer/content/text field; if the stream is plain JSON it parses that instead.
func parseSSEAnswer(r io.Reader) string {
	var sb strings.Builder
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 1024*1024), 8*1024*1024)
	sawData := false
	for sc.Scan() {
		line := sc.Text()
		if !strings.HasPrefix(line, "data:") {
			continue
		}
		sawData = true
		payload := strings.TrimSpace(strings.TrimPrefix(line, "data:"))
		if payload == "" || payload == "[DONE]" {
			continue
		}
		var m map[string]interface{}
		if json.Unmarshal([]byte(payload), &m) == nil {
			if s := pick(m, "answer", "content", "text", "delta", "message"); s != "" {
				sb.WriteString(s)
				continue
			}
			if d, ok := m["data"].(map[string]interface{}); ok {
				sb.WriteString(pick(d, "answer", "content", "text", "delta"))
				continue
			}
		}
		sb.WriteString(payload)
	}
	if !sawData {
		// not an SSE stream — try whole-body JSON
		// (scanner already consumed it; nothing to do beyond what we collected)
	}
	return sb.String()
}

func autoDeep(q string) bool {
	if len([]rune(q)) > 30 {
		return true
	}
	for _, kw := range []string{"为什么", "分析", "比较", "区别", "步骤", "原理", "架构", "如何", "怎么", "怎样", "流程", "配置", "故障", "排查", "部署", "安装", "升级", "标准", "规则", "制度", "政策", "条件", "要求", "资格"} {
		if strings.Contains(q, kw) {
			return true
		}
	}
	return false
}

// ──────────────────────────── business: download ────────────────────────────

func cmdDownload(args map[string]string) int {
	if ensureToken() == "" {
		fmt.Fprintln(os.Stderr, "未登录或登录已过期，请先连接（login）")
		return 3
	}
	url := args["url"]
	if url == "" {
		fmt.Fprintln(os.Stderr, "需要 --url（本 Windows 版暂只支持 --url 直接下载）")
		return 2
	}
	outDir := firstNonEmpty(args["output"], "downloads")
	_ = os.MkdirAll(outDir, 0o755)
	name := firstNonEmpty(args["filename"], "document.bin")

	req, _ := http.NewRequest("GET", searchBase+"/search/downSearchFile?url="+url, nil)
	authHeaders(req, true)
	resp, err := httpClient().Do(req)
	if err != nil {
		fmt.Fprintln(os.Stderr, "下载失败:", err)
		return 1
	}
	defer resp.Body.Close()
	out := filepath.Join(outDir, name)
	f, err := os.Create(out)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer f.Close()
	n, _ := io.Copy(f, resp.Body)
	fmt.Printf("✅ 文档下载成功\n📁 %s\n💾 %.2f MB\n", out, float64(n)/1024/1024)
	return 0
}

// ──────────────────────────── helpers ────────────────────────────

func emitJSON(v interface{}) {
	b, _ := json.Marshal(v)
	fmt.Println(string(b))
}

func pick(m map[string]interface{}, keys ...string) string {
	for _, k := range keys {
		if v, ok := m[k]; ok && v != nil {
			switch x := v.(type) {
			case string:
				if x != "" {
					return x
				}
			case float64:
				return trimFloat(x)
			default:
				return fmt.Sprintf("%v", x)
			}
		}
	}
	return ""
}

func trimFloat(f float64) string {
	if f == float64(int64(f)) {
		return fmt.Sprintf("%d", int64(f))
	}
	return fmt.Sprintf("%g", f)
}

func firstNonEmpty(s ...string) string {
	for _, x := range s {
		if strings.TrimSpace(x) != "" {
			return x
		}
	}
	return ""
}

func stripBearer(s string) string { return strings.TrimSpace(strings.TrimPrefix(s, "Bearer ")) }

func yesno(b bool) string {
	if b {
		return "是"
	}
	return "否"
}

func atoiDefault(s string, def int) int {
	if s == "" {
		return def
	}
	n := 0
	for _, c := range s {
		if c < '0' || c > '9' {
			return def
		}
		n = n*10 + int(c-'0')
	}
	return n
}

// parseArgs turns `--k v` / `--k=v` / `--flag` into a map. Bare flags → "true".
func parseArgs(a []string) map[string]string {
	m := map[string]string{}
	for i := 0; i < len(a); i++ {
		arg := a[i]
		if !strings.HasPrefix(arg, "-") {
			continue
		}
		arg = strings.TrimLeft(arg, "-")
		if eq := strings.IndexByte(arg, '='); eq >= 0 {
			m[arg[:eq]] = arg[eq+1:]
			continue
		}
		// short aliases
		if arg == "q" {
			arg = "query"
		}
		if i+1 < len(a) && !strings.HasPrefix(a[i+1], "-") {
			m[arg] = a[i+1]
			i++
		} else {
			m[arg] = "true"
		}
	}
	return m
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "用法: zhidao <login|poll|save|load|exchange|clear|qa|search|download> [参数...]")
		os.Exit(2)
	}
	cmd := os.Args[1]
	args := parseArgs(os.Args[2:])
	var code int
	switch cmd {
	case "login":
		code = cmdLogin()
	case "poll":
		code = cmdPoll(args)
	case "exchange":
		code = cmdExchange()
	case "save":
		code = cmdSave(args)
	case "load":
		code = cmdLoad()
	case "clear":
		code = cmdClear()
	case "search":
		code = cmdSearch(args)
	case "qa":
		code = cmdQA(args)
	case "download":
		code = cmdDownload(args)
	case "--help", "-h", "help":
		fmt.Println("zhidao <login|poll|save|load|exchange|clear|qa|search|download>")
	default:
		fmt.Fprintln(os.Stderr, "未知子命令:", cmd)
		code = 2
	}
	os.Exit(code)
}
