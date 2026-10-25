// Real isolated TLS peer. Private keys remain in memory and never enter output.
package main

import (
	"bufio"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"io"
	"log"
	"math/big"
	"net"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"
)

type wireCase struct{ Operation, Path, Request, Response string }
type observation struct {
	Path, Method, Host, Body, AcceptEncoding, ContentType, CacheControl string
	HasAuthorization, HasCookie, HasProxyAuthorization                  bool
}
type records struct {
	sync.Mutex
	Requests                []observation
	ProxyHits, RedirectHits int
}

func certificate() (tls.Certificate, string) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		panic("fixture key unavailable")
	}
	serial, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	if err != nil {
		panic("fixture serial unavailable")
	}
	root := &x509.Certificate{SerialNumber: serial, Subject: pkix.Name{CommonName: "isolated control fixture"}, NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour), IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign | x509.KeyUsageDigitalSignature}
	der, err := x509.CreateCertificate(rand.Reader, root, root, &key.PublicKey, key)
	if err != nil {
		panic("fixture root unavailable")
	}
	leafKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		panic("fixture leaf unavailable")
	}
	leaf := &x509.Certificate{SerialNumber: new(big.Int).Add(serial, big.NewInt(1)), DNSNames: []string{"update.pinvou.com"}, NotBefore: root.NotBefore, NotAfter: root.NotAfter, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}}
	leafDER, err := x509.CreateCertificate(rand.Reader, leaf, root, &leafKey.PublicKey, key)
	if err != nil {
		panic("fixture leaf certificate unavailable")
	}
	return tls.Certificate{Certificate: [][]byte{leafDER, der}, PrivateKey: leafKey}, string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}))
}
func main() {
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		os.Exit(1)
	}
	var cases []wireCase
	if json.Unmarshal(data, &cases) != nil || len(cases) != 27 {
		os.Exit(1)
	}
	rootData, err := os.ReadFile(os.Args[3])
	if err != nil {
		os.Exit(1)
	}
	var roots map[string]string
	if json.Unmarshal(rootData, &roots) != nil || roots["1"] == "" || roots["2"] == "" {
		os.Exit(1)
	}
	state := &records{}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		os.Exit(1)
	}
	proxy, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		os.Exit(1)
	}
	cert, root := certificate()
	_, untrusted := certificate()
	proxyServer := &http.Server{ErrorLog: log.New(io.Discard, "", 0), Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		state.Lock()
		state.ProxyHits++
		state.Unlock()
		w.WriteHeader(407)
	})}
	go proxyServer.Serve(proxy)
	server := &http.Server{ErrorLog: log.New(io.Discard, "", 0), Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/redirected" {
			state.Lock()
			state.RedirectHits++
			state.Unlock()
			w.WriteHeader(500)
			return
		}
		body, err := io.ReadAll(io.LimitReader(r.Body, 1048577))
		if err != nil {
			w.WriteHeader(500)
			return
		}
		state.Lock()
		state.Requests = append(state.Requests, observation{
			Path: r.URL.Path, Method: r.Method, Host: r.Host, Body: string(body),
			AcceptEncoding: r.Header.Get("Accept-Encoding"), ContentType: r.Header.Get("Content-Type"), CacheControl: r.Header.Get("Cache-Control"),
			HasAuthorization: r.Header.Get("Authorization") != "", HasCookie: r.Header.Get("Cookie") != "", HasProxyAuthorization: r.Header.Get("Proxy-Authorization") != ""})
		state.Unlock()
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("Set-Cookie", "synthetic-fixture=not-a-real-secret; Secure; HttpOnly")
		if r.Method == http.MethodGet {
			rootGet(w, r, roots)
			return
		}
		for _, c := range cases {
			if c.Path == r.URL.Path && c.Request == string(body) {
				io.WriteString(w, c.Response)
				return
			}
		}
		var v map[string]any
		if json.Unmarshal(body, &v) != nil {
			w.WriteHeader(500)
			return
		}
		mode, _ := v["transactionId"].(string)
		mode = strings.TrimPrefix(mode, "case-")
		result := []byte("{\"protocolVersion\":1,\"requestId\":\"fixture-1\",\"state\":\"consumed\",\"transactionId\":\"transaction-1\"}")
		if mode == "cl-same" || mode == "cl-conflict" || mode == "cl-plus" || mode == "te-cl" || mode == "te-encoding" || mode == "te-duplicate" || mode == "truncated" {
			connection, writer, err := w.(http.Hijacker).Hijack()
			if err != nil {
				return
			}
			defer connection.Close()
			fmt.Fprint(writer, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n")
			n := len(result)
			switch mode {
			case "cl-same":
				fmt.Fprintf(writer, "Content-Length: %d\r\nContent-Length: %d\r\n", n, n)
			case "cl-conflict":
				fmt.Fprintf(writer, "Content-Length: %d\r\nContent-Length: %d\r\n", n, n+1)
			case "cl-plus":
				fmt.Fprintf(writer, "Content-Length: +%d\r\n", n)
			case "te-cl":
				fmt.Fprintf(writer, "Transfer-Encoding: chunked\r\nContent-Length: %d\r\n", n)
			case "te-encoding":
				fmt.Fprint(writer, "Transfer-Encoding: gzip\r\n")
			case "te-duplicate":
				fmt.Fprint(writer, "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n")
			case "truncated":
				fmt.Fprintf(writer, "Content-Length: %d\r\n", n+1)
			}
			fmt.Fprint(writer, "\r\n")
			if mode == "te-cl" || mode == "te-duplicate" {
				fmt.Fprintf(writer, "%x\r\n%s\r\n0\r\n\r\n", n, result)
			} else {
				writer.Write(result)
			}
			writer.Flush()
			return
		}
		switch mode {
		case "redirect":
			w.Header().Set("Location", "/redirected")
			w.WriteHeader(302)
		case "foreign-redirect":
			w.Header().Set("Location", "https://redirect.test.invalid/redirected")
			w.WriteHeader(307)
		case "status":
			w.WriteHeader(201)
		case "connection-drop":
			connection, _, err := w.(http.Hijacker).Hijack()
			if err == nil {
				connection.Close()
			}
			return
		case "progress":
			w.WriteHeader(200)
			w.(http.Flusher).Flush()
			for offset := 0; offset < len(result); offset += 4 {
				end := offset + 4
				if end > len(result) {
					end = len(result)
				}
				w.Write(result[offset:end])
				w.(http.Flusher).Flush()
				time.Sleep(50 * time.Millisecond)
			}
			return
		case "charset":
			w.Header().Set("Content-Type", "application/json; charset=\"utf-8")
		case "chunk-valid":
			w.WriteHeader(200)
			w.(http.Flusher).Flush()
		case "media":
			w.Header().Set("Content-Type", "text/html")
		case "gzip":
			w.Header().Set("Content-Encoding", "gzip")
		case "oversize":
			w.Header().Set("Content-Length", "1048577")
			result = []byte(" ")
		case "chunk-oversize":
			w.WriteHeader(200)
			w.(http.Flusher).Flush()
			io.WriteString(w, strings.Repeat(" ", 1048577))
			return
		case "duplicate":
			result = []byte("{\"protocolVersion\":1,\"protocolVersion\":1}")
		case "cancel":
			if os.WriteFile(os.Args[2], []byte("seen"), 0600) != nil {
				w.WriteHeader(500)
				return
			}
			select {
			case <-r.Context().Done():
				return
			case <-time.After(2 * time.Second):
			}
		case "headers-wait":
			select {
			case <-r.Context().Done():
				return
			case <-time.After(2 * time.Second):
			}
		case "body-wait":
			w.WriteHeader(200)
			w.(http.Flusher).Flush()
			select {
			case <-r.Context().Done():
				return
			case <-time.After(2 * time.Second):
			}
		case "declared-error":
			w.WriteHeader(404)
			result = []byte("{\"protocolVersion\":1,\"requestId\":\"fixture-1\",\"code\":\"UNKNOWN_CONSUME_OUTCOME\",\"retryable\":false}")
		case "exact-cap":
			result = append(result, []byte(strings.Repeat(" ", 1048576-len(result)))...)
		case "restore": // Valid body after every negative; no cookie is sent back.
		default:
			w.WriteHeader(500)
			return
		}
		w.Write(result)
	})}
	go server.Serve(tls.NewListener(listener, &tls.Config{Certificates: []tls.Certificate{cert}, MinVersion: tls.VersionTLS12}))
	json.NewEncoder(os.Stdout).Encode(map[string]string{"address": listener.Addr().String(), "proxyAddress": proxy.Addr().String(), "certificatePem": root, "untrustedCertificatePem": untrusted, "cancellationSeenPath": os.Args[2]})
	bufio.NewReader(os.Stdin).ReadString('\n')
	server.Close()
	proxyServer.Close()
	state.Lock()
	defer state.Unlock()
	if json.NewEncoder(os.Stdout).Encode(state) != nil {
		fmt.Fprintln(os.Stderr, "fixture output unavailable")
		os.Exit(1)
	}
}

// Isolated GET fixtures are deliberately structural DATA, never Root authority.
// Only pinvou/1 and pinvou/2 are original public signed vector bytes.
func rootGet(w http.ResponseWriter, r *http.Request, roots map[string]string) {
	parts := strings.Split(r.URL.Path, "/")
	if len(parts) != 6 || parts[1] != "v1" || parts[2] != "upgrade" || parts[3] != "roots" {
		w.WriteHeader(500)
		return
	}
	product := parts[4]
	version := strings.TrimSuffix(parts[5], ".json")
	if product == "pinvou" {
		if version == "3" {
			w.WriteHeader(404)
			io.WriteString(w, `{"protocolVersion":1,"code":"ROOT_NOT_FOUND"}`)
			return
		}
		if raw, ok := roots[version]; ok {
			io.WriteString(w, raw)
			return
		}
		w.WriteHeader(500)
		return
	}
	raw := []byte(roots["1"])
	switch product {
	case "redirect":
		w.Header().Set("Location", "/redirected")
		w.WriteHeader(302)
		return
	case "foreign-redirect":
		w.Header().Set("Location", "https://redirect.test.invalid/redirected")
		w.WriteHeader(307)
		return
	case "status":
		w.WriteHeader(503)
		return
	case "gzip":
		w.Header().Set("Content-Encoding", "gzip")
	case "media":
		w.Header().Set("Content-Type", "text/html")
	case "oversize":
		w.Header().Set("Content-Length", "1048577")
		raw = []byte(" ")
	case "chunk-oversize":
		w.WriteHeader(200)
		w.(http.Flusher).Flush()
		raw = []byte(strings.Repeat(" ", 1048577))
	case "body-wait":
		w.WriteHeader(200)
		w.(http.Flusher).Flush()
		select {
		case <-r.Context().Done():
			return
		case <-time.After(2 * time.Second):
		}
	case "noncanonical":
		raw = append(raw, '\n')
	case "wrong-context": // Original pinvou Root deliberately differs from selector.
	case "bad-missing":
		w.WriteHeader(404)
		io.WriteString(w, `{"protocolVersion":1,"code":"ROOT_NOT_FOUND","latest":true}`)
		return
	case "truncated":
		connection, writer, err := w.(http.Hijacker).Hijack()
		if err != nil {
			return
		}
		defer connection.Close()
		fmt.Fprintf(writer, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\nConnection: close\r\n\r\n", len(raw)+1)
		writer.Write(raw)
		writer.Flush()
		return
	case "connection-drop":
		connection, _, err := w.(http.Hijacker).Hijack()
		if err == nil {
			connection.Close()
		}
		return
	case "progress":
		var value map[string]any
		if json.Unmarshal(raw, &value) != nil {
			w.WriteHeader(500)
			return
		}
		value["signed"].(map[string]any)["product"] = "progress"
		raw, _ = json.Marshal(value) // ASCII-only fixture keys/values; lexical canonical order.
		w.WriteHeader(200)
		w.(http.Flusher).Flush()
		for offset := 0; offset < len(raw); offset += 128 {
			end := offset + 128
			if end > len(raw) {
				end = len(raw)
			}
			if _, err := w.Write(raw[offset:end]); err != nil {
				return
			}
			w.(http.Flusher).Flush()
			time.Sleep(50 * time.Millisecond)
		}
		return
	default:
		w.WriteHeader(500)
		return
	}
	w.Write(raw)
}
