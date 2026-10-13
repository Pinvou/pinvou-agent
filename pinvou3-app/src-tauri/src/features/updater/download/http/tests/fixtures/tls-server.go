// Isolated test peer. TLS private material is generated in memory and never
// written to a file, returned by HTTP, or emitted to stdout/stderr.
package main

import (
	"bufio"
	"compress/gzip"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"io"
	"log"
	"math/big"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"
)

type observation struct {
	Path                  string `json:"path"`
	Method                string `json:"method"`
	AcceptEncoding        string `json:"acceptEncoding"`
	Range                 string `json:"range"`
	IfRange               string `json:"ifRange"`
	HasAuthorization      bool   `json:"hasAuthorization"`
	HasCookie             bool   `json:"hasCookie"`
	HasProxyAuthorization bool   `json:"hasProxyAuthorization"`
}
type records struct {
	mu           sync.Mutex
	Requests     []observation `json:"requests"`
	ProxyHits    int           `json:"proxyHits"`
	RedirectHits int           `json:"redirectHits"`
}

func main() {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		os.Exit(1)
	}
	serial, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	if err != nil {
		os.Exit(1)
	}
	template := &x509.Certificate{
		SerialNumber: serial,
		Subject:      pkix.Name{CommonName: "isolated file transport fixture"},
		DNSNames:     []string{"file.test.invalid"},
		NotBefore:    time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour),
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true, IsCA: true,
	}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		os.Exit(1)
	}
	leafKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		os.Exit(1)
	}
	leaf := &x509.Certificate{
		SerialNumber: new(big.Int).Add(serial, big.NewInt(1)),
		Subject:      pkix.Name{CommonName: "file.test.invalid"}, DNSNames: []string{"file.test.invalid"},
		NotBefore: template.NotBefore, NotAfter: template.NotAfter,
		KeyUsage:              x509.KeyUsageDigitalSignature,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
		BasicConstraintsValid: true,
	}
	leafDER, err := x509.CreateCertificate(rand.Reader, leaf, template, &leafKey.PublicKey, key)
	if err != nil {
		os.Exit(1)
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		os.Exit(1)
	}
	proxy, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		listener.Close()
		os.Exit(1)
	}
	state := &records{}
	proxyServer := &http.Server{ErrorLog: log.New(io.Discard, "", 0), Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		state.mu.Lock()
		state.ProxyHits++
		state.mu.Unlock()
		w.Header().Set("Proxy-Authenticate", "Basic realm=fixture")
		w.WriteHeader(http.StatusProxyAuthRequired)
	})}
	go proxyServer.Serve(proxy)
	server := &http.Server{ErrorLog: log.New(io.Discard, "", 0), Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/inspect" {
			state.mu.Lock()
			defer state.mu.Unlock()
			json.NewEncoder(w).Encode(state)
			return
		}
		state.mu.Lock()
		state.Requests = append(state.Requests, observation{
			Path: r.URL.Path, Method: r.Method,
			AcceptEncoding: r.Header.Get("Accept-Encoding"), Range: r.Header.Get("Range"), IfRange: r.Header.Get("If-Range"),
			HasAuthorization: r.Header.Get("Authorization") != "", HasCookie: r.Header.Get("Cookie") != "", HasProxyAuthorization: r.Header.Get("Proxy-Authorization") != "",
		})
		if r.URL.Path == "/redirect-target" {
			state.RedirectHits++
		}
		state.mu.Unlock()
		if strings.HasPrefix(r.URL.Path, "/redirect/") {
			status, _ := strconv.Atoi(strings.TrimPrefix(r.URL.Path, "/redirect/"))
			w.Header().Set("Location", "/redirect-target")
			w.WriteHeader(status)
			return
		}
		if strings.HasPrefix(r.URL.Path, "/framing/") {
			connection, buffered, err := w.(http.Hijacker).Hijack()
			if err != nil {
				return
			}
			defer connection.Close()
			coding := "chunked"
			if r.URL.Path == "/framing/coding" {
				coding = "gzip, chunked"
			}
			fmt.Fprintf(buffered, "HTTP/1.1 200 OK\r\nTransfer-Encoding: %s\r\n", coding)
			if r.URL.Path == "/framing/conflict" {
				fmt.Fprint(buffered, "Content-Length: 10\r\n")
			}
			if r.URL.Path == "/framing/duplicate" {
				fmt.Fprint(buffered, "Transfer-Encoding: chunked\r\n")
			}
			fmt.Fprint(buffered, "\r\na\r\n0123456789\r\n0\r\n\r\n")
			buffered.Flush()
			return
		}
		if strings.HasPrefix(r.URL.Path, "/duplicates/") {
			connection, buffered, err := w.(http.Hijacker).Hijack()
			if err != nil {
				return
			}
			defer connection.Close()
			fmt.Fprint(buffered, "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n")
			switch r.URL.Path {
			case "/duplicates/length-equal":
				fmt.Fprint(buffered, "Content-Length: 10\r\n")
			case "/duplicates/length-conflict":
				fmt.Fprint(buffered, "Content-Length: 9\r\n")
			case "/duplicates/range":
				fmt.Fprint(buffered, "Content-Range: bytes 0-9/10\r\nContent-Range: bytes 0-9/10\r\n")
			case "/duplicates/encoding":
				fmt.Fprint(buffered, "Content-Encoding: identity\r\nContent-Encoding: identity\r\n")
			}
			fmt.Fprint(buffered, "\r\n0123456789")
			buffered.Flush()
			return
		}
		switch r.URL.Path {
		case "/precondition":
			w.WriteHeader(412)
			return
		case "/unsatisfiable":
			w.Header().Set("Content-Range", "bytes */10")
			w.WriteHeader(416)
			return
		case "/cookie":
			w.Header().Set("Set-Cookie", "fixture=opaque; Secure; Path=/")
		case "/gzip":
			w.Header().Set("Content-Encoding", "gzip")
			writer := gzip.NewWriter(w)
			writer.Write([]byte("0123456789"))
			writer.Close()
			return
		case "/br":
			w.Header().Set("Content-Encoding", "br")
			w.Write([]byte("encoded"))
			return
		case "/duplicate-etag":
			w.Header().Add("ETag", "\"original\"")
			w.Header().Add("ETag", "\"original\"")
		case "/overlong":
			w.(http.Flusher).Flush()
			w.Write([]byte("01234567890"))
			return
		case "/truncated":
			w.(http.Flusher).Flush()
			w.Write([]byte("012345678"))
			return
		case "/network-error":
			w.Header().Set("Content-Length", "10")
			w.Write([]byte("0123"))
			w.(http.Flusher).Flush()
			connection, _, hijackErr := w.(http.Hijacker).Hijack()
			if hijackErr == nil {
				connection.Close()
			}
			return
		case "/stall":
			w.Header().Set("Content-Length", "10")
			w.(http.Flusher).Flush()
			time.Sleep(500 * time.Millisecond)
			w.Write([]byte("0123456789"))
			return
		case "/slow-progress":
			w.Header().Set("Content-Length", "10")
			w.(http.Flusher).Flush()
			for _, value := range []byte("0123456789") {
				w.Write([]byte{value})
				w.(http.Flusher).Flush()
				time.Sleep(40 * time.Millisecond)
			}
			return
		case "/large-chunk":
			w.Header().Set("Content-Length", "131073")
			w.Write([]byte(strings.Repeat("x", 131073)))
			return
		}
		if r.URL.Path == "/partial" || r.URL.Path == "/short-segment" || strings.HasPrefix(r.URL.Path, "/wrong-") {
			start := 4
			if r.Header.Get("Range") == "bytes=7-" {
				start = 7
			}
			end := 9
			if r.URL.Path == "/short-segment" {
				end = 6
			}
			total := 10
			if r.URL.Path == "/wrong-total" {
				total = 11
			}
			if r.URL.Path == "/wrong-offset" {
				start++
			}
			tag := "\"original\""
			if r.URL.Path == "/wrong-etag" {
				tag = "\"other\""
			}
			w.Header().Set("ETag", tag)
			w.Header().Set("Content-Range", fmt.Sprintf("bytes %d-%d/%d", start, end, total))
			length := end - start + 1
			if r.URL.Path == "/wrong-length" {
				length++
			}
			w.Header().Set("Content-Length", strconv.Itoa(length))
			w.WriteHeader(206)
			w.Write([]byte("0123456789"[start : end+1]))
			return
		}
		if r.URL.Path != "/duplicate-etag" {
			w.Header().Set("ETag", "\"original\"")
		}
		w.Header().Set("Content-Length", "10")
		w.Write([]byte("0123456789"))
	})}
	certificate := tls.Certificate{Certificate: [][]byte{leafDER, der}, PrivateKey: leafKey}
	tlsListener := tls.NewListener(listener, &tls.Config{Certificates: []tls.Certificate{certificate}, MinVersion: tls.VersionTLS12})
	go server.Serve(tlsListener)
	digest := sha256.Sum256([]byte("0123456789"))
	json.NewEncoder(os.Stdout).Encode(map[string]any{
		"address": listener.Addr().String(), "proxyAddress": proxy.Addr().String(),
		"origin":         fmt.Sprintf("https://file.test.invalid:%d", listener.Addr().(*net.TCPAddr).Port),
		"certificatePem": string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})),
		"sha256":         hex.EncodeToString(digest[:]),
	})
	// External harness closes stdin after the Rust tests. No private state exits.
	bufio.NewReader(os.Stdin).ReadBytes('\n')
	server.Close()
	proxyServer.Close()
	state.mu.Lock()
	json.NewEncoder(os.Stdout).Encode(state)
	state.mu.Unlock()
}
