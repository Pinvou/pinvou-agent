(function (root) {
  "use strict";

  function hasFiles(event) {
    return !!(event.dataTransfer
      && Array.prototype.indexOf.call(event.dataTransfer.types || [], "Files") >= 0);
  }

  function install(options) {
    var doc = options.document || root.document;
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
      if (dragDepth === 1) setActive(true);
    }

    function onDragOver(event) {
      if (!canAccept() || !hasFiles(event)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
      if (dragDepth === 0) {
        dragDepth = 1;
        setActive(true);
      }
    }

    function onDragLeave() {
      if (dragDepth === 0) return;
      dragDepth -= 1;
      if (dragDepth === 0) setActive(false);
    }

    function onDrop(event) {
      var files = event.dataTransfer && event.dataTransfer.files;
      if (!canAccept() || !files || files.length === 0) return;
      event.preventDefault();
      dragDepth = 0;
      setActive(false);
      var droppedFiles = Array.prototype.slice.call(files);
      Promise.resolve(options.onFiles(droppedFiles)).catch(function (error) {
        console.warn("[attachment] dropped file processing failed", error);
      });
    }

    doc.addEventListener("dragenter", onDragEnter);
    doc.addEventListener("dragover", onDragOver);
    doc.addEventListener("dragleave", onDragLeave);
    doc.addEventListener("drop", onDrop);

    return function uninstall() {
      doc.removeEventListener("dragenter", onDragEnter);
      doc.removeEventListener("dragover", onDragOver);
      doc.removeEventListener("dragleave", onDragLeave);
      doc.removeEventListener("drop", onDrop);
      dragDepth = 0;
      setActive(false);
    };
  }

  root.PinvouAttachmentDropController = Object.freeze({
    install: install,
  });
})(window);
