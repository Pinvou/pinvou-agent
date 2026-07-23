/** Remote desktop-host file picker used by the browser WebUI. */
(function () {
  "use strict";

  if (!window.PinvouPlatform || window.PinvouPlatform.kind !== "web") return;
  var client = window.PinvouWebClient;
  if (!client) return;

  var activePicker = null;

  function element(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function entryIsDirectory(entry) {
    return entry && (entry.is_dir === true || entry.isDir === true || entry.kind === "directory" || entry.kind === "root");
  }

  function allowedByFilters(entry, filters) {
    if (entryIsDirectory(entry) || !filters || !filters.length) return true;
    var extensions = [];
    filters.forEach(function (filter) {
      (filter.extensions || []).forEach(function (extension) {
        extensions.push(String(extension).replace(/^\./, "").toLowerCase());
      });
    });
    if (!extensions.length) return true;
    var name = String(entry.name || "");
    var extension = name.indexOf(".") >= 0 ? name.split(".").pop().toLowerCase() : "";
    return extensions.indexOf(extension) >= 0;
  }

  function formatSize(value) {
    var size = Number(value || 0);
    if (!size) return "";
    if (size < 1024) return size + " B";
    if (size < 1024 * 1024) return (size / 1024).toFixed(1) + " KB";
    if (size < 1024 * 1024 * 1024) return (size / 1024 / 1024).toFixed(1) + " MB";
    return (size / 1024 / 1024 / 1024).toFixed(1) + " GB";
  }

  function openRemoteHostPicker(options) {
    options = options || {};
    if (activePicker) return Promise.reject(new Error("已有文件选择器正在打开"));

    return new Promise(function (resolve, reject) {
      var directoryMode = options.directory === true;
      var multiple = !directoryMode && options.multiple === true;
      var selected = new Map();
      var currentPath = null;
      var parentPath = null;
      var disposed = false;
      var loadGeneration = 0;

      var overlay = element("div", "pinvou-host-picker-overlay");
      var panel = element("div", "pinvou-host-picker-panel");
      var header = element("div", "pinvou-host-picker-header");
      var heading = element("div", "pinvou-host-picker-heading", options.title || (directoryMode ? "选择桌面端文件夹" : "选择桌面端文件"));
      var close = element("button", "pinvou-host-picker-icon", "×");
      close.type = "button";
      close.setAttribute("aria-label", "关闭");
      header.appendChild(heading);
      header.appendChild(close);

      var toolbar = element("div", "pinvou-host-picker-toolbar");
      var up = element("button", "pinvou-host-picker-icon", "←");
      up.type = "button";
      up.title = "上一级";
      var pathLabel = element("div", "pinvou-host-picker-path", "正在读取桌面端目录…");
      toolbar.appendChild(up);
      toolbar.appendChild(pathLabel);

      var body = element("div", "pinvou-host-picker-body");
      var status = element("div", "pinvou-host-picker-status", "正在读取…");
      body.appendChild(status);

      var footer = element("div", "pinvou-host-picker-footer");
      var selectionLabel = element("div", "pinvou-host-picker-selection", "");
      var actions = element("div", "pinvou-host-picker-actions");
      var cancel = element("button", "pinvou-host-picker-button", "取消");
      var confirm = element("button", "pinvou-host-picker-button pinvou-host-picker-primary", directoryMode ? "选择此文件夹" : "选择");
      cancel.type = confirm.type = "button";
      confirm.disabled = !directoryMode;
      actions.appendChild(cancel);
      actions.appendChild(confirm);
      footer.appendChild(selectionLabel);
      footer.appendChild(actions);

      panel.appendChild(header);
      panel.appendChild(toolbar);
      panel.appendChild(body);
      panel.appendChild(footer);
      overlay.appendChild(panel);
      document.body.appendChild(overlay);
      activePicker = overlay;

      function finish(value, error) {
        if (disposed) return;
        disposed = true;
        activePicker = null;
        window.removeEventListener("keydown", onKeyDown);
        overlay.remove();
        if (error) reject(error);
        else resolve(value);
      }

      function updateSelection() {
        var count = selected.size;
        selectionLabel.textContent = directoryMode
          ? (currentPath ? "当前文件夹：" + currentPath : "")
          : (count ? "已选择 " + count + " 项" : "");
        confirm.disabled = directoryMode ? !currentPath : count === 0;
      }

      function chooseEntry(entry, row) {
        if (entryIsDirectory(entry)) {
          load(entry.path);
          return;
        }
        if (!multiple) {
          selected.clear();
          Array.prototype.forEach.call(body.querySelectorAll(".is-selected"), function (item) {
            item.classList.remove("is-selected");
          });
        }
        if (selected.has(entry.path)) {
          selected.delete(entry.path);
          row.classList.remove("is-selected");
        } else {
          selected.set(entry.path, entry);
          row.classList.add("is-selected");
        }
        updateSelection();
      }

      function renderListing(listing) {
        currentPath = listing && (listing.path || listing.current_path || listing.currentPath) || null;
        parentPath = listing && (listing.parent || listing.parent_path || listing.parentPath) || null;
        pathLabel.textContent = currentPath || "此电脑";
        up.disabled = !parentPath;
        body.replaceChildren();

        var entries = [];
        if (listing && Array.isArray(listing.roots) && !parentPath) {
          entries = entries.concat(listing.roots.map(function (root) {
            return Object.assign({ is_dir: true, kind: "root" }, root);
          }));
        }
        if (listing && Array.isArray(listing.entries)) entries = entries.concat(listing.entries);
        entries = entries.filter(function (entry) { return allowedByFilters(entry, options.filters); });
        entries.sort(function (a, b) {
          var ad = entryIsDirectory(a) ? 0 : 1;
          var bd = entryIsDirectory(b) ? 0 : 1;
          return ad - bd || String(a.name || "").localeCompare(String(b.name || ""), "zh-CN");
        });

        if (!entries.length) {
          body.appendChild(element("div", "pinvou-host-picker-empty", "此目录中没有可选内容"));
        }
        entries.forEach(function (entry) {
          var row = element("button", "pinvou-host-picker-row");
          row.type = "button";
          var icon = element("span", "pinvou-host-picker-file-icon", entryIsDirectory(entry) ? "📁" : "📄");
          var name = element("span", "pinvou-host-picker-name", entry.name || entry.path || "");
          var size = element("span", "pinvou-host-picker-size", entryIsDirectory(entry) ? "" : formatSize(entry.size));
          row.appendChild(icon);
          row.appendChild(name);
          row.appendChild(size);
          if (selected.has(entry.path)) row.classList.add("is-selected");
          row.addEventListener("click", function () { chooseEntry(entry, row); });
          row.addEventListener("dblclick", function () {
            if (entryIsDirectory(entry)) return;
            selected.set(entry.path, entry);
            finish(multiple ? Array.from(selected.keys()) : entry.path);
          });
          body.appendChild(row);
        });
        updateSelection();
      }

      function load(path) {
        var generation = ++loadGeneration;
        up.disabled = true;
        confirm.disabled = true;
        body.replaceChildren(element("div", "pinvou-host-picker-status", "正在读取桌面端目录…"));
        client.invoke("web_access_list_host_files", { path: path || null }).then(function (listing) {
          if (disposed || generation !== loadGeneration) return;
          renderListing(listing);
        }).catch(function (error) {
          if (disposed || generation !== loadGeneration) return;
          body.replaceChildren(element("div", "pinvou-host-picker-error", "读取失败：" + String(error && error.message ? error.message : error)));
        });
      }

      function onKeyDown(event) {
        if (event.key === "Escape") finish(null);
      }

      close.addEventListener("click", function () { finish(null); });
      cancel.addEventListener("click", function () { finish(null); });
      up.addEventListener("click", function () { if (parentPath) load(parentPath); });
      confirm.addEventListener("click", function () {
        if (directoryMode) finish(currentPath);
        else finish(multiple ? Array.from(selected.keys()) : Array.from(selected.keys())[0] || null);
      });
      window.addEventListener("keydown", onKeyDown);
      load(null);
    });
  }

  var style = document.createElement("style");
  style.textContent = [
    ".pinvou-host-picker-overlay{position:fixed;inset:0;z-index:300;display:flex;align-items:center;justify-content:center;padding:16px;background:rgba(0,0,0,.55);backdrop-filter:blur(6px);font-family:-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif}",
    ".pinvou-host-picker-panel{display:flex;flex-direction:column;width:min(720px,100%);height:min(680px,88vh);overflow:hidden;border:1px solid #3c4043;border-radius:20px;background:#202124;color:#e8eaed;box-shadow:0 24px 80px rgba(0,0,0,.5)}",
    ".pinvou-host-picker-header,.pinvou-host-picker-toolbar,.pinvou-host-picker-footer{display:flex;align-items:center;padding:12px 16px;border-bottom:1px solid #3c4043}",
    ".pinvou-host-picker-header{justify-content:space-between}.pinvou-host-picker-heading{font-size:17px;font-weight:650}",
    ".pinvou-host-picker-icon{display:grid;place-items:center;width:36px;height:36px;border:0;border-radius:50%;background:transparent;color:inherit;font-size:22px;cursor:pointer}.pinvou-host-picker-icon:hover{background:#303134}.pinvou-host-picker-icon:disabled{opacity:.35;cursor:default}",
    ".pinvou-host-picker-toolbar{gap:10px;padding-block:8px}.pinvou-host-picker-path{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#bdc1c6;font-size:13px}",
    ".pinvou-host-picker-body{flex:1;overflow:auto;padding:8px}.pinvou-host-picker-row{display:grid;grid-template-columns:28px minmax(0,1fr) auto;align-items:center;gap:8px;width:100%;min-height:44px;padding:7px 10px;border:0;border-radius:10px;background:transparent;color:inherit;text-align:left;cursor:pointer}.pinvou-host-picker-row:hover{background:#303134}.pinvou-host-picker-row.is-selected{background:#394457;color:#d2e3fc}",
    ".pinvou-host-picker-name{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:14px}.pinvou-host-picker-size{color:#9aa0a6;font-size:12px}.pinvou-host-picker-status,.pinvou-host-picker-empty,.pinvou-host-picker-error{padding:28px;text-align:center;color:#9aa0a6;font-size:13px}.pinvou-host-picker-error{color:#f28b82}",
    ".pinvou-host-picker-footer{justify-content:space-between;gap:12px;border-top:1px solid #3c4043;border-bottom:0}.pinvou-host-picker-selection{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#9aa0a6;font-size:12px}.pinvou-host-picker-actions{display:flex;gap:8px}.pinvou-host-picker-button{height:38px;padding:0 18px;border:1px solid #5f6368;border-radius:19px;background:transparent;color:#e8eaed;font-weight:600;cursor:pointer}.pinvou-host-picker-button:hover{background:#303134}.pinvou-host-picker-primary{border-color:#8ab4f8;background:#8ab4f8;color:#202124}.pinvou-host-picker-button:disabled{opacity:.4;cursor:default}",
    "html:not(.dark) .pinvou-host-picker-panel{border-color:#dadce0;background:#fff;color:#202124}html:not(.dark) .pinvou-host-picker-header,html:not(.dark) .pinvou-host-picker-toolbar,html:not(.dark) .pinvou-host-picker-footer{border-color:#dadce0}html:not(.dark) .pinvou-host-picker-row:hover{background:#f1f3f4}html:not(.dark) .pinvou-host-picker-row.is-selected{background:#d2e3fc;color:#174ea6}",
    "@media(max-width:600px){.pinvou-host-picker-overlay{align-items:stretch;padding:0}.pinvou-host-picker-panel{width:100%;height:100%;max-height:none;border:0;border-radius:0}.pinvou-host-picker-footer{padding-bottom:max(12px,env(safe-area-inset-bottom))}.pinvou-host-picker-selection{display:none}}",
  ].join("");
  document.head.appendChild(style);

  window.PinvouHostFilePicker = { open: openRemoteHostPicker };
  if (window.__TAURI__ && window.__TAURI__.dialog) {
    window.__TAURI__.dialog.open = openRemoteHostPicker;
  }
})();
