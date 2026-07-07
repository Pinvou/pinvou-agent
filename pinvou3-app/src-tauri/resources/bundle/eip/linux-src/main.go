package main

import (
	"bytes"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

const (
	authBaseDefault = "http://api.eos.h3c.com/auth/v1.0"
	eipBaseDefault  = "http://apieip.h3c.com/eip"
	credSalt        = "eip-cli-cred-v1"
	userAgent       = "eip-cli (pinvou3 linux arm64)"
)

type envelope struct {
	Code        any             `json:"code"`
	Msg         string          `json:"msg"`
	Message     string          `json:"message"`
	Data        json.RawMessage `json:"data"`
	SsoLoginURL string          `json:"ssoLoginUrl"`
	SessionID   string          `json:"sessionId"`
	Token       string          `json:"token"`
	Ait         string          `json:"ait"`
}

type sessionData struct {
	SessionID   string `json:"sessionId"`
	SsoLoginURL string `json:"ssoLoginUrl"`
	Token       string `json:"token"`
	Ait         string `json:"ait"`
}

type route struct {
	method string
	path   string
}

var routes = map[string]route{
	"attendance get-summary":            {"POST", "/api/ApiCalendarMonthly/ApplyFlowPageList"},
	"attendance get-calendar":           {"POST", "/api/ApiAttendance/GetEmpCalendarDetailInfo"},
	"attendance get-overview":           {"POST", "/api/ApiAttendance/GetEmpMonthSummaryInfo"},
	"attendance get-annual-days":        {"POST", "/api/ApiAttendance/GetAnnualDays"},
	"attendance get-schedule":           {"POST", "/api/ApiAttendance/GetWorkSchduleList"},
	"attendance list-shuttle-bus":       {"POST", "/api/ApiAttendance/GetShuttleBusLineList"},
	"attendance list-leave-types":       {"POST", "/api/apiAttendance/GetLeaveApplyTypeList"},
	"attendance calc-days":              {"POST", "/api/ApiAttendance/CalcApplyItemDays"},
	"attendance get-annual-calc":        {"POST", "/api/ApiAttendance/GetAnnualCaclData"},
	"attendance list-abnormal":          {"POST", "/api/ApiSapWorkAttendance/GetAbTimeRecordTable"},
	"attendance list-forget-card":       {"POST", "/api/ApiSapWorkAttendance/GetEmpWSKCS"},
	"attendance list-forget-badge":      {"POST", "/api/ApiSapWorkAttendance/GetEmpWDKCS"},
	"attendance list-flex":              {"POST", "/api/ApiSapWorkAttendance/GetEmpOCLAbnormalData"},
	"attendance get-person-summary":     {"POST", "/api/ApiAttendance/GetPersonCaclAttendanceModelList"},
	"attendance get-effectived-summary": {"POST", "/api/ApiSapWorkAttendance/GetWorkAttendanceTotalTable"},
	"leave get-balance":                 {"POST", "/api/ApiAttendance/GetAnnualLeaveBalance"},
	"leave get-other-balance":           {"POST", "/api/ApiAttendance/GetOtherLeaveBalance"},
	"leave get-sick-balance":            {"POST", "/api/ApiAttendance/GetFullPaySickLeaveBalance"},
	"leave get-sick-used":               {"POST", "/api/ApiAttendance/GetSikeLeaveSAPHours"},
	"leave get-compassionate-used":      {"POST", "/api/ApiAttendance/GetCompassionateLeaveApprovedDays"},
	"vacation list-annual":              {"POST", "/api/ApiSapWorkAttendance/GetVacationInfoTableList"},
	"vacation list-sick":                {"POST", "/api/ApiSapWorkAttendance/GetVacationInfoLeaveTableList"},
	"vacation list-compensatory":        {"POST", "/api/ApiSapWorkAttendance/GetWorkVacationLeaveList"},
	"vacation get-remaining-hours":      {"POST", "/api/ApiSapWorkAttendance/GetEmpTXCS"},
	"overtime list-types":               {"POST", "/api/ApiAttendance/GetOvertimeApplyTypeList"},
	"overtime get-extra-hours":          {"POST", "/api/ApiAttendance/GetOvertimeApplyExtraHours"},
	"overtime get-approving-hours":      {"POST", "/api/ApiAttendance/GetOvertimeApplyApprovingHours"},
	"overtime get-approved-hours":       {"POST", "/api/ApiAttendance/GetOvertimeApplyApprovedHours"},
	"overtime get-effectived-hours":     {"POST", "/api/ApiAttendance/GetOvertimeApplyEffectivedHours"},
	"overtime get-limit-hours":          {"POST", "/api/ApiAttendance/GetOvertimeLimitHours"},
	"todo":                              {"POST", "/api/ApiAttendance/GetWorkFlowList"},
	"todo-count":                        {"POST", "/api/ApiAttendance/GetWorkFlowList"},
	"todo-detail":                       {"POST", "/api/ApiAttendance/GetApplyWorkFlowInfo"},
	"apply list":                        {"POST", "/api/ApiCalendarMonthly/ApplyFlowPageList"},
	"apply get-detail":                  {"POST", "/api/ApiAttendance/GetApplyWorkFlowInfo"},
	"apply list-attachments":            {"POST", "/api/ApiAttendance/GetApplyBillAttachFileList"},
	"employee-profile":                  {"POST", "/api/ApiStaffNavigation/NewStaffStatus"},
	"employee-contact":                  {"POST", "/organisation/user/tel/info/%s"},
	"employee-handbook sign":            {"POST", "/api/ApiStaffNavigation/SignForReceipt"},
	"employee-search":                   {"POST", "/api/ApiStaffNavigation/NewStaffStatus"},
	"commercial-insurance":              {"POST", "/api/ApiStaffNavigation/GetMyInsuranceList"},
	"contract":                          {"POST", "/api/ApiBenefits/MyContractPageList"},
	"consumption monthly":               {"POST", "/api/ApiMyConsumption/MyConsumptionMonthly"},
	"consumption list":                  {"POST", "/api/ApiMyConsumption/MyConsumptionPageList"},
	"consumption trend":                 {"POST", "/api/ApiMyConsumption/MyConsumptionMonthlyList"},
	"expense recent":                    {"POST", "/api/ApiCalendarMonthly/ApplyFlowPageList"},
	"fuel-card list":                    {"POST", "/api/ApiBenefits/MyOilCardPageList"},
	"fuel-card seasons":                 {"POST", "/api/ApiBenefits/OilCardInvestSeasonly"},
	"fuel-card apply-records":           {"POST", "/api/ApiBenefits/OilCardApplyRecords"},
	"union-welfare":                     {"POST", "/api/ApiBenefits/MyUnionWelfarePageList"},
	"union-apply join":                  {"POST", "/api/ApiStaffNavigation/JoinUnion"},
	"union-apply status":                {"POST", "/api/ApiStaffNavigation/UnionApply"},
	"survey":                            {"POST", "/api/ApiOpinionTemplate/GetOpenOpinionTemplateList"},
	"survey detail":                     {"POST", "/api/ApiOpinionTemplate/OpinionCollection/%s"},
	"survey submit":                     {"POST", "/api/ApiOpinionTemplate/SaveOpinionCollectionInfo"},
	"remote-work":                       {"POST", "/api/ApiCalendarMonthly/ApplyFlowPageList"},
}

func main() {
	os.Exit(run(os.Args[1:]))
}

func run(args []string) int {
	args = stripGlobalFlags(args)
	if len(args) == 0 || args[0] == "help" || args[0] == "--help" || args[0] == "-h" {
		printHelp()
		return 0
	}
	if args[0] == "version" {
		emitJSON(map[string]any{"name": "eip-cli", "version": "0.1.0-arm64-pinvou3"})
		return 0
	}
	if args[0] == "config-get" {
		emitJSON(map[string]any{"authBase": authBase(), "eipBase": eipBase(), "deviceId": deviceID()})
		return 0
	}
	if args[0] == "config" {
		if len(args) > 1 && args[1] == "device-id" {
			fmt.Println(deviceID())
			return 0
		}
		emitJSON(map[string]any{"ok": true, "message": "config set-env is not persisted by the ARM64 fallback"})
		return 0
	}
	if args[0] == "auth" {
		return runAuth(args[1:])
	}
	return runBusiness(args)
}

func stripGlobalFlags(args []string) []string {
	out := make([]string, 0, len(args))
	for i := 0; i < len(args); i++ {
		a := args[i]
		switch a {
		case "--debug", "--verbose", "--non-interactive":
			continue
		case "--output", "--config":
			i++
			continue
		default:
			if strings.HasPrefix(a, "--output=") || strings.HasPrefix(a, "--config=") {
				continue
			}
			out = append(out, a)
		}
	}
	return out
}

func runAuth(args []string) int {
	if len(args) == 0 || args[0] == "--help" || args[0] == "-h" {
		fmt.Println("eip-cli auth <login|poll|save|status|logout>")
		return 0
	}
	flags := parseFlags(args[1:])
	switch args[0] {
	case "login":
		return cmdLogin(flags)
	case "poll":
		return cmdPoll(flags)
	case "save":
		return cmdSave(flags)
	case "status":
		emitJSON(map[string]any{"hasToken": loadAT() != "", "hasAIT": loadAIT() != "", "sessionId": loadSessionID()})
		return 0
	case "logout":
		_ = os.Remove(atFile())
		_ = os.Remove(aitFile())
		_ = os.Remove(sessionFile())
		emitJSON(map[string]any{"ok": true})
		return 0
	default:
		emitJSON(map[string]any{"error": "unknown auth command: " + args[0]})
		return 2
	}
}

func cmdLogin(flags map[string]string) int {
	name := firstNonEmpty(flags["name"], "eip-cli-agent")
	body := map[string]any{
		"deviceId": deviceID(),
		"metaData": map[string]string{"name": name, "os": "linux", "arch": "arm64", "client": "pinvou3"},
	}
	env, raw, err := postJSON(authBase()+"/api/v2/user/agent/session/login", body, false)
	if err != nil {
		emitJSON(map[string]any{"error": err.Error()})
		return 1
	}
	d := env.session()
	if d.SessionID != "" {
		_ = os.WriteFile(sessionFile(), []byte(d.SessionID), 0o600)
	}
	if d.SsoLoginURL == "" {
		emitJSON(map[string]any{"error": "login did not return ssoLoginUrl", "message": firstNonEmpty(env.Msg, env.Message), "raw": string(raw)})
		return 1
	}
	emitJSON(map[string]any{"ssoLoginUrl": d.SsoLoginURL, "sessionId": d.SessionID, "message": "open ssoLoginUrl to finish SSO"})
	if boolFlag(flags, "no-poll") {
		return 0
	}
	return cmdPoll(map[string]string{"session-id": d.SessionID, "timeout": firstNonEmpty(flags["timeout"], "30")})
}

func cmdPoll(flags map[string]string) int {
	sid := firstNonEmpty(flags["session-id"], loadSessionID())
	if sid == "" {
		emitJSON(map[string]any{"error": "missing --session-id"})
		return 2
	}
	timeout := atoiDefault(flags["timeout"], 5)
	deadline := time.Now().Add(time.Duration(timeout) * time.Second)
	for {
		env, _, err := getURL(authBase()+"/api/v2/user/agent/session/poll/"+url.PathEscape(sid), false)
		if err == nil {
			d := env.session()
			if d.Token != "" {
				_ = saveAT(d.Token)
				_ = saveAIT(d.Ait)
				emitJSON(map[string]any{"hasToken": true, "connected": true})
				return 0
			}
		}
		if time.Now().After(deadline) {
			emitJSON(map[string]any{"connected": false, "pending": true})
			return 8
		}
		time.Sleep(2 * time.Second)
	}
}

func cmdSave(flags map[string]string) int {
	if raw := flags["json"]; raw != "" {
		var env envelope
		if err := json.Unmarshal([]byte(raw), &env); err != nil {
			emitJSON(map[string]any{"error": err.Error()})
			return 2
		}
		d := env.session()
		flags["token"] = firstNonEmpty(flags["token"], d.Token)
		flags["ait"] = firstNonEmpty(flags["ait"], d.Ait)
	}
	if flags["token"] == "" && flags["ait"] == "" {
		emitJSON(map[string]any{"error": "missing --token or --ait"})
		return 2
	}
	if flags["token"] != "" {
		if err := saveAT(stripBearer(flags["token"])); err != nil {
			emitJSON(map[string]any{"error": err.Error()})
			return 1
		}
	}
	if flags["ait"] != "" {
		if err := saveAIT(flags["ait"]); err != nil {
			emitJSON(map[string]any{"error": err.Error()})
			return 1
		}
	}
	emitJSON(map[string]any{"ok": true})
	return 0
}

// authExpired 判定响应是否为 token 失效/未授权，需刷新后重试。
// EIP 业务接口对过期 token 返回 HTTP 200 + {"Code":51,"Msg":"Token已失效"}；
// 个别接口走 HTTP 401 或以文案表达，一并兜底。
func authExpired(raw []byte, err error) bool {
	if err != nil {
		return strings.Contains(err.Error(), "HTTP 401")
	}
	var probe struct {
		Code json.Number `json:"Code"`
		Msg  string      `json:"Msg"`
	}
	if json.Unmarshal(raw, &probe) == nil {
		if probe.Code.String() == "51" {
			return true
		}
		return strings.Contains(probe.Msg, "失效") ||
			strings.Contains(probe.Msg, "未登录") ||
			strings.Contains(probe.Msg, "登录已过期")
	}
	return strings.Contains(string(raw), "Token已失效") ||
		strings.Contains(string(raw), "登录已过期")
}

func runBusiness(args []string) int {
	key := args[0]
	rest := args[1:]
	if len(rest) > 0 && !strings.HasPrefix(rest[0], "-") {
		key = key + " " + rest[0]
		rest = rest[1:]
	}
	r, ok := routes[key]
	if !ok {
		emitJSON(map[string]any{"error": "unsupported EIP command in ARM64 fallback", "command": strings.Join(args, " ")})
		return 2
	}
	token := ensureToken()
	if token == "" {
		emitJSON(map[string]any{"error": "not authenticated", "message": "please connect EIP first"})
		return 3
	}
	flags := parseFlags(rest)
	body := buildBody(flags)
	path := r.path
	if strings.Contains(path, "%s") {
		fill := firstNonEmpty(flags["account"], flags["id"], flags["template-id"])
		if fill == "" {
			emitJSON(map[string]any{"error": "missing required path parameter"})
			return 2
		}
		path = fmt.Sprintf(path, url.PathEscape(fill))
	}
	raw, err := callEIP(r.method, path, body, token)
	// token 过期(服务端 Code 51 / "失效"，个别接口 HTTP 401）→ 清 AT、用 AIT 刷新、重试一次。
	// 对齐真 eip-cli 的自动刷新:此前 ensureToken 只在 AT 为空时才刷新，存量 AT 过期会被原样发出去遭拒。
	if authExpired(raw, err) {
		_ = os.Remove(atFile())
		if nt := ensureToken(); nt != "" && nt != token {
			raw, err = callEIP(r.method, path, body, nt)
		}
	}
	if err != nil {
		emitJSON(map[string]any{"error": err.Error()})
		return 1
	}
	os.Stdout.Write(raw)
	if len(raw) == 0 || raw[len(raw)-1] != '\n' {
		fmt.Println()
	}
	return 0
}

func buildBody(flags map[string]string) map[string]any {
	now := time.Now()
	body := map[string]any{}
	for k, v := range flags {
		body[k] = coerce(v)
		body[toCamel(k)] = coerce(v)
	}
	if _, ok := body["page"]; !ok {
		body["page"] = 1
	}
	if _, ok := body["pageSize"]; !ok {
		body["pageSize"] = 10
	}
	if d, ok := body["date"].(string); ok && len(d) == 7 {
		body["month"] = d
		body["caclDate"] = d
		body["calcDate"] = d
	}
	if _, ok := body["date"]; !ok {
		body["date"] = now.Format("2006-01")
	}
	if _, ok := body["year"]; !ok {
		body["year"] = now.Format("2006")
	}
	if _, ok := body["begTime"]; !ok {
		body["begTime"] = "08:30:00"
	}
	if _, ok := body["endTime"]; !ok {
		body["endTime"] = "18:00:00"
	}
	return body
}

func callEIP(method, path string, body map[string]any, token string) ([]byte, error) {
	full := strings.TrimRight(eipBase(), "/") + apiPrefix(path) + path
	var reader io.Reader
	if method == "POST" {
		b, _ := json.Marshal(body)
		reader = bytes.NewReader(b)
	}
	req, err := http.NewRequest(method, full, reader)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "application/json, text/plain, */*")
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", userAgent)
	if !strings.HasPrefix(token, "Bearer ") {
		token = "Bearer " + token
	}
	req.Header.Set("Authorization", token)
	resp, err := httpClient().Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode >= 400 {
		return raw, fmt.Errorf("EIP API %s returned HTTP %d: %s", path, resp.StatusCode, string(raw))
	}
	return raw, nil
}

func authBase() string { return firstNonEmpty(os.Getenv("EIP_AUTH_BASE_URL"), authBaseDefault) }
func eipBase() string  { return firstNonEmpty(os.Getenv("EIP_BASE_URL"), eipBaseDefault) }

func apiPrefix(path string) string {
	switch {
	case strings.HasPrefix(path, "/organisation/"):
		return "/hr"
	case strings.HasPrefix(path, "/api/ApiStaffNavigation/"),
		strings.HasPrefix(path, "/api/ApiBenefits/"),
		strings.HasPrefix(path, "/api/ApiOpinionTemplate/"),
		strings.HasPrefix(path, "/api/ApiMyConsumption/"):
		return "/hr"
	default:
		return "/hrss"
	}
}

func httpClient() *http.Client {
	return &http.Client{Timeout: 60 * time.Second, Transport: &http.Transport{Proxy: nil}}
}

func postJSON(uri string, body any, withToken bool) (*envelope, []byte, error) {
	b, _ := json.Marshal(body)
	req, err := http.NewRequest("POST", uri, bytes.NewReader(b))
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

func getURL(uri string, withToken bool) (*envelope, []byte, error) {
	req, err := http.NewRequest("GET", uri, nil)
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

func authHeaders(req *http.Request, withToken bool) {
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")
	req.Header.Set("Auth-Type", "SSO_NEXT")
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

func (e *envelope) session() sessionData {
	var d sessionData
	if len(e.Data) > 0 {
		_ = json.Unmarshal(e.Data, &d)
	}
	if d.SessionID == "" {
		d.SessionID = e.SessionID
	}
	if d.SsoLoginURL == "" {
		d.SsoLoginURL = e.SsoLoginURL
	}
	if d.Token == "" {
		d.Token = e.Token
	}
	if d.Ait == "" {
		d.Ait = e.Ait
	}
	return d
}

func deviceID() string {
	if v := strings.TrimSpace(os.Getenv("AGENT_DEVICE_ID")); v != "" {
		return v
	}
	return "eip-cli-default-device"
}

func credDir() string {
	d := os.Getenv("AGENT_CREDENTIALS_DIR")
	if d == "" {
		home, _ := os.UserHomeDir()
		d = filepath.Join(home, ".pinvou3", "eip", "credentials")
	}
	_ = os.MkdirAll(d, 0o700)
	return d
}

func atFile() string      { return filepath.Join(credDir(), "credentials_at.enc") }
func aitFile() string     { return filepath.Join(credDir(), "credentials_ait.enc") }
func sessionFile() string { return filepath.Join(credDir(), "session_id") }

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
		return "", err
	}
	return string(plain), nil
}

func loadAT() string        { s, _ := decryptFromFile(atFile()); return s }
func loadAIT() string       { s, _ := decryptFromFile(aitFile()); return s }
func saveAT(v string) error { return encryptToFile(atFile(), stripBearer(v)) }
func saveAIT(v string) error {
	if v == "" {
		return nil
	}
	return encryptToFile(aitFile(), v)
}
func loadSessionID() string {
	b, _ := os.ReadFile(sessionFile())
	return strings.TrimSpace(string(b))
}

func ensureToken() string {
	if t := loadAT(); t != "" {
		return t
	}
	ait := loadAIT()
	if ait == "" {
		return ""
	}
	body := map[string]any{"ait": ait, "deviceId": deviceID()}
	env, _, err := postJSON(authBase()+"/api/v2/user/agent/token/exchange", body, false)
	if err != nil {
		return ""
	}
	d := env.session()
	if d.Token == "" {
		return ""
	}
	_ = saveAT(d.Token)
	if d.Ait != "" {
		_ = saveAIT(d.Ait)
	}
	return d.Token
}

func parseFlags(args []string) map[string]string {
	m := map[string]string{}
	for i := 0; i < len(args); i++ {
		a := args[i]
		if !strings.HasPrefix(a, "--") {
			continue
		}
		a = strings.TrimPrefix(a, "--")
		if eq := strings.IndexByte(a, '='); eq >= 0 {
			m[a[:eq]] = a[eq+1:]
			continue
		}
		if i+1 < len(args) && !strings.HasPrefix(args[i+1], "-") {
			m[a] = args[i+1]
			i++
		} else {
			m[a] = "true"
		}
	}
	return m
}

func boolFlag(m map[string]string, k string) bool {
	v := strings.ToLower(m[k])
	return v == "true" || v == "1" || v == "yes"
}

func coerce(v string) any {
	if i, err := strconv.Atoi(v); err == nil {
		return i
	}
	if b, err := strconv.ParseBool(v); err == nil {
		return b
	}
	return v
}

func toCamel(s string) string {
	parts := strings.Split(s, "-")
	for i := 1; i < len(parts); i++ {
		if parts[i] != "" {
			parts[i] = strings.ToUpper(parts[i][:1]) + parts[i][1:]
		}
	}
	return strings.Join(parts, "")
}

func atoiDefault(s string, d int) int {
	if v, err := strconv.Atoi(s); err == nil && v > 0 {
		return v
	}
	return d
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if strings.TrimSpace(v) != "" {
			return v
		}
	}
	return ""
}

func stripBearer(s string) string {
	return strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(s), "Bearer "))
}

func emitJSON(v any) {
	b, _ := json.MarshalIndent(v, "", "  ")
	fmt.Println(string(b))
}

func printHelp() {
	fmt.Println("EIP CLI ARM64 fallback")
	fmt.Println("Usage: eip-cli <auth|attendance|leave|vacation|overtime|todo|employee-profile|employee-search|...> [flags]")
}
