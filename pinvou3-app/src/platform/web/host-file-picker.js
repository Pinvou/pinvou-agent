/** Remote desktop-host file picker used by the browser WebUI. */
(function () {
  "use strict";

  if (!window.PinvouPlatform || window.PinvouPlatform.kind !== "web") return;
  var client = window.PinvouWebClient;
  if (!client) return;

  var activePicker = null;

  // 文案单一来源是 shared/i18n.js 的 uiPlatformMisc.hostFilePicker,由 React 入口
  // 按当前语言挂到 window.PinvouHostFilePickerStrings;此处保留中文兜底(纯脚本无法 import ES module)。
  var FALLBACK_LABELS = {
    pickFolderTitle: "选择桌面端文件夹", pickFileTitle: "选择桌面端文件",
    close: "关闭", goUp: "上一级", loadingPath: "正在读取桌面端目录…", loading: "正在读取…",
    cancel: "取消", chooseThisFolder: "选择此文件夹", choose: "选择",
    currentFolder: function (path) { return "当前文件夹：" + path; },
    selectedCount: function (n) { return "已选择 " + n + " 项"; },
    thisComputer: "此电脑", home: "用户目录", emptyFolder: "此目录中没有可选内容",
    loadFailed: function (err) { return "读取失败：" + err; },
    workspaceNotAuthorized: "请先在桌面端允许远程访问本机目录",
    alreadyOpen: "已有文件选择器正在打开",
  };

  function pickerLabels() {
    var custom = window.PinvouHostFilePickerStrings || {};
    var merged = {};
    for (var key in FALLBACK_LABELS) {
      merged[key] = custom[key] !== undefined ? custom[key] : FALLBACK_LABELS[key];
    }
    return merged;
  }

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
    var labels = pickerLabels();
    if (activePicker) return Promise.reject(new Error(labels.alreadyOpen));

    return new Promise(function (resolve, reject) {
      var directoryMode = options.directory === true;
      var multiple = !directoryMode && options.multiple === true;
      var selected = new Map();
      var currentPath = null;
      var parentPath = null;
      var currentWorkspaceHandle = null;
      var rootEntries = [];
      var showingRoots = false;
      var disposed = false;
      var loadGeneration = 0;
      var initialPath = options.defaultPath || null;
      var initialPathPending = Boolean(initialPath);

      var overlay = element("div", "pinvou-host-picker-overlay");
      var panel = element("div", "pinvou-host-picker-panel");
      var header = element("div", "pinvou-host-picker-header");
      var heading = element("div", "pinvou-host-picker-heading", options.title || (directoryMode ? labels.pickFolderTitle : labels.pickFileTitle));
      var close = element("button", "pinvou-host-picker-icon", "×");
      close.type = "button";
      close.setAttribute("aria-label", labels.close);
      header.appendChild(heading);
      header.appendChild(close);

      var toolbar = element("div", "pinvou-host-picker-toolbar");
      var rootsButton = element("button", "pinvou-host-picker-root-button", labels.thisComputer);
      rootsButton.type = "button";
      rootsButton.title = labels.thisComputer;
      rootsButton.disabled = true;
      var up = element("button", "pinvou-host-picker-icon", "←");
      up.type = "button";
      up.title = labels.goUp;
      var pathLabel = element("div", "pinvou-host-picker-path", labels.loadingPath);
      toolbar.appendChild(rootsButton);
      toolbar.appendChild(up);
      toolbar.appendChild(pathLabel);

      var body = element("div", "pinvou-host-picker-body");
      var status = element("div", "pinvou-host-picker-status", labels.loading);
      body.appendChild(status);

      var footer = element("div", "pinvou-host-picker-footer");
      var selectionLabel = element("div", "pinvou-host-picker-selection", "");
      var actions = element("div", "pinvou-host-picker-actions");
      var cancel = element("button", "pinvou-host-picker-button", labels.cancel);
      var confirm = element("button", "pinvou-host-picker-button pinvou-host-picker-primary", directoryMode ? labels.chooseThisFolder : labels.choose);
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
          ? (currentPath ? labels.currentFolder(currentPath) : "")
          : (count ? labels.selectedCount(count) : "");
        confirm.disabled = directoryMode
          ? (!currentPath || (options.workspaceGrant === true && !currentWorkspaceHandle))
          : count === 0;
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

      function renderEntries(entries, preserveOrder) {
        body.replaceChildren();
        entries = entries.filter(function (entry) { return allowedByFilters(entry, options.filters); });
        if (!preserveOrder) {
          entries.sort(function (a, b) {
            var ad = entryIsDirectory(a) ? 0 : 1;
            var bd = entryIsDirectory(b) ? 0 : 1;
            return ad - bd || String(a.name || "").localeCompare(String(b.name || ""), "zh-CN");
          });
        }

        if (!entries.length) {
          body.appendChild(element("div", "pinvou-host-picker-empty", labels.emptyFolder));
        }
        entries.forEach(function (entry) {
          var row = element("button", "pinvou-host-picker-row");
          row.type = "button";
          var icon = element("span", "pinvou-host-picker-file-icon", entryIsDirectory(entry) ? "📁" : "📄");
          var displayName = entry.kind === "root" && entry.name === "Home"
            ? labels.home
            : (entry.name || entry.path || "");
          var name = element("span", "pinvou-host-picker-name", displayName);
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

      function rememberRoots(listing) {
        if (!listing || !Array.isArray(listing.roots)) return;
        rootEntries = listing.roots.filter(function (root) {
          return root && root.path;
        }).map(function (root) {
          return Object.assign({}, root, { is_dir: true, kind: "root" });
        });
      }

      function showRoots() {
        if (!rootEntries.length) return;
        loadGeneration += 1;
        showingRoots = true;
        currentPath = null;
        parentPath = null;
        currentWorkspaceHandle = null;
        pathLabel.textContent = labels.thisComputer;
        rootsButton.disabled = false;
        up.disabled = true;
        renderEntries(rootEntries.slice(), true);
      }

      function renderListing(listing) {
        rememberRoots(listing);
        showingRoots = false;
        currentPath = listing && (listing.path || listing.current_path || listing.currentPath) || null;
        parentPath = listing && (listing.parent || listing.parent_path || listing.parentPath) || null;
        currentWorkspaceHandle = listing
          && (listing.workspace_handle || listing.workspaceHandle) || null;
        pathLabel.textContent = currentPath || labels.thisComputer;
        rootsButton.disabled = rootEntries.length === 0;
        up.disabled = !parentPath && rootEntries.length === 0;
        renderEntries(listing && Array.isArray(listing.entries) ? listing.entries.slice() : [], false);
      }

      function load(path) {
        var generation = ++loadGeneration;
        showingRoots = false;
        currentWorkspaceHandle = null;
        rootsButton.disabled = rootEntries.length === 0;
        up.disabled = true;
        confirm.disabled = true;
        body.replaceChildren(element("div", "pinvou-host-picker-status", labels.loadingPath));
        client.invoke("web_access_list_host_files", {
          path: path || null,
          issueWorkspaceHandle: options.workspaceGrant === true,
        }).then(function (listing) {
          if (disposed || generation !== loadGeneration) return;
          if (initialPathPending && path === initialPath) initialPathPending = false;
          renderListing(listing);
        }).catch(function (error) {
          if (disposed || generation !== loadGeneration) return;
          if (initialPathPending && path === initialPath) {
            initialPathPending = false;
            load(null);
            return;
          }
          rootsButton.disabled = rootEntries.length === 0;
          var message = String(error && error.message ? error.message : error);
          if (message.indexOf("host_workspace_not_authorized") >= 0) message = labels.workspaceNotAuthorized;
          body.replaceChildren(element("div", "pinvou-host-picker-error", labels.loadFailed(message)));
        });
      }

      function onKeyDown(event) {
        if (event.key === "Escape") finish(null);
      }

      close.addEventListener("click", function () { finish(null); });
      cancel.addEventListener("click", function () { finish(null); });
      rootsButton.addEventListener("click", showRoots);
      up.addEventListener("click", function () {
        if (parentPath) load(parentPath);
        else if (!showingRoots) showRoots();
      });
      confirm.addEventListener("click", function () {
        if (directoryMode && options.workspaceGrant === true) {
          finish({ path: currentPath, workspaceHandle: currentWorkspaceHandle });
        } else if (directoryMode) finish(currentPath);
        else finish(multiple ? Array.from(selected.keys()) : Array.from(selected.keys())[0] || null);
      });
      window.addEventListener("keydown", onKeyDown);
      load(initialPath);
    });
  }

  var style = document.createElement("style");
  style.textContent = [
    ".pinvou-host-picker-overlay{position:fixed;inset:0;z-index:300;display:flex;align-items:center;justify-content:center;padding:16px;background:rgba(0,0,0,.55);backdrop-filter:blur(6px);font-family:-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif}",
    ".pinvou-host-picker-panel{display:flex;flex-direction:column;width:min(720px,100%);height:min(680px,88vh);overflow:hidden;border:1px solid #3c4043;border-radius:20px;background:#202124;color:#e8eaed;box-shadow:0 24px 80px rgba(0,0,0,.5)}",
    ".pinvou-host-picker-header,.pinvou-host-picker-toolbar,.pinvou-host-picker-footer{display:flex;align-items:center;padding:12px 16px;border-bottom:1px solid #3c4043}",
    ".pinvou-host-picker-header{justify-content:space-between}.pinvou-host-picker-heading{font-size:17px;font-weight:650}",
    ".pinvou-host-picker-icon{display:grid;place-items:center;width:36px;height:36px;border:0;border-radius:50%;background:transparent;color:inherit;font-size:22px;cursor:pointer}.pinvou-host-picker-icon:hover{background:#303134}.pinvou-host-picker-icon:disabled{opacity:.35;cursor:default}",
    ".pinvou-host-picker-toolbar{gap:10px;padding-block:8px}.pinvou-host-picker-root-button{flex:none;height:32px;padding:0 12px;border:0;border-radius:16px;background:transparent;color:inherit;font-size:13px;cursor:pointer}.pinvou-host-picker-root-button:hover{background:#303134}.pinvou-host-picker-root-button:disabled{opacity:.45;cursor:default}.pinvou-host-picker-path{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#bdc1c6;font-size:13px}",
    ".pinvou-host-picker-body{flex:1;overflow:auto;padding:8px}.pinvou-host-picker-row{display:grid;grid-template-columns:28px minmax(0,1fr) auto;align-items:center;gap:8px;width:100%;min-height:44px;padding:7px 10px;border:0;border-radius:10px;background:transparent;color:inherit;text-align:left;cursor:pointer}.pinvou-host-picker-row:hover{background:#303134}.pinvou-host-picker-row.is-selected{background:#394457;color:#d2e3fc}",
    ".pinvou-host-picker-name{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:14px}.pinvou-host-picker-size{color:#9aa0a6;font-size:12px}.pinvou-host-picker-status,.pinvou-host-picker-empty,.pinvou-host-picker-error{padding:28px;text-align:center;color:#9aa0a6;font-size:13px}.pinvou-host-picker-error{color:#f28b82}",
    ".pinvou-host-picker-footer{justify-content:space-between;gap:12px;border-top:1px solid #3c4043;border-bottom:0}.pinvou-host-picker-selection{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:#9aa0a6;font-size:12px}.pinvou-host-picker-actions{display:flex;gap:8px}.pinvou-host-picker-button{height:38px;padding:0 18px;border:1px solid #5f6368;border-radius:19px;background:transparent;color:#e8eaed;font-weight:600;cursor:pointer}.pinvou-host-picker-button:hover{background:#303134}.pinvou-host-picker-primary{border-color:#8ab4f8;background:#8ab4f8;color:#202124}.pinvou-host-picker-button:disabled{opacity:.4;cursor:default}",
    "html:not(.dark) .pinvou-host-picker-panel{border-color:#dadce0;background:#fff;color:#202124}html:not(.dark) .pinvou-host-picker-header,html:not(.dark) .pinvou-host-picker-toolbar,html:not(.dark) .pinvou-host-picker-footer{border-color:#dadce0}html:not(.dark) .pinvou-host-picker-root-button:hover,html:not(.dark) .pinvou-host-picker-row:hover{background:#f1f3f4}html:not(.dark) .pinvou-host-picker-row.is-selected{background:#d2e3fc;color:#174ea6}",
    "@media(max-width:600px){.pinvou-host-picker-overlay{align-items:stretch;padding:0}.pinvou-host-picker-panel{width:100%;height:100%;max-height:none;border:0;border-radius:0}.pinvou-host-picker-footer{padding-bottom:max(12px,env(safe-area-inset-bottom))}.pinvou-host-picker-selection{display:none}}",
  ].join("");
  document.head.appendChild(style);

  window.PinvouHostFilePicker = {
    open: openRemoteHostPicker,
    openWorkspace: function (options) {
      return openRemoteHostPicker(Object.assign({}, options || {}, {
        directory: true,
        multiple: false,
        workspaceGrant: true,
      }));
    },
  };
  if (window.__TAURI__ && window.__TAURI__.dialog) {
    window.__TAURI__.dialog.open = openRemoteHostPicker;
  }
})();
