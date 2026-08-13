(function (root) {
  "use strict";

  function hasFiles(event) {
    return !!(event.dataTransfer
      && Array.prototype.indexOf.call(event.dataTransfer.types || [], "Files") >= 0);
  }

  function install(options) {
    var doc = options.document || root.document;
    // capture: true 时在捕获阶段监听并在已受理路径 stopPropagation —— 供
    // 工具市场等子视图在 document 上接管文件拖放,隔离全局附件通道
    // (doc 冒泡阶段监听仍会收到事件)。
    var capture = options.capture === true;
    var dragDepth = 0;
    var active = false;

    function canAccept() {
      return !options.canAccept || options.canAccept() === true;
    }

    function setActive(next) {
      next = !!next;
      if (active === next) return;
      active = next;
      options.onActiveChange(next);
    }

    function onDragEnter(event) {
      if (!canAccept() || !hasFiles(event)) return;
      dragDepth += 1;
      event.preventDefault();
      if (capture) event.stopPropagation();
      if (dragDepth === 1) setActive(true);
    }

    function onDragOver(event) {
      if (!canAccept() || !hasFiles(event)) return;
      event.preventDefault();
      if (capture) event.stopPropagation();
      event.dataTransfer.dropEffect = "copy";
      if (dragDepth === 0) {
        dragDepth = 1;
        setActive(true);
      }
    }

    function onDragLeave(event) {
      if (dragDepth === 0) return;
      dragDepth -= 1;
      if (capture) event.stopPropagation();
      if (dragDepth === 0) setActive(false);
    }

    function onDrop(event) {
      var files = event.dataTransfer && event.dataTransfer.files;
      if (!canAccept() || !files || files.length === 0) return;
      event.preventDefault();
      if (capture) event.stopPropagation();
      dragDepth = 0;
      setActive(false);
      var droppedFiles = Array.prototype.slice.call(files);
      Promise.resolve(options.onFiles(droppedFiles)).catch(function (error) {
        console.warn("[attachment] dropped file processing failed", error);
      });
    }

    doc.addEventListener("dragenter", onDragEnter, capture);
    doc.addEventListener("dragover", onDragOver, capture);
    doc.addEventListener("dragleave", onDragLeave, capture);
    doc.addEventListener("drop", onDrop, capture);

    return function uninstall() {
      doc.removeEventListener("dragenter", onDragEnter, capture);
      doc.removeEventListener("dragover", onDragOver, capture);
      doc.removeEventListener("dragleave", onDragLeave, capture);
      doc.removeEventListener("drop", onDrop, capture);
      dragDepth = 0;
      setActive(false);
    };
  }

  root.PinvouAttachmentDropController = Object.freeze({
    install: install,
  });
})(window);
