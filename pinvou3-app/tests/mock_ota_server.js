#!/usr/bin/env node
const http = require('http');

const port = Number(process.env.MOCK_OTA_PORT || 8788);

const json = (res, payload) => {
  res.setHeader('Content-Type', 'application/json; charset=utf-8');
  res.end(JSON.stringify(payload));
};

const server = http.createServer((req, res) => {
  req.resume();
  req.on('end', () => {
    if (req.url === '/v2/bootstrap') {
      json(res, {
        code: 200,
        success: true,
        msg: '操作成功',
        data: { smarthubOta: `http://127.0.0.1:${port}` },
      });
      return;
    }

    if (req.url === '/ota/pkg/package/upgrade/check' || req.url === '/ota/pkg/package/upgrade/getDownloadInfo') {
      json(res, {
        code: 200,
        success: true,
        msg: '操作成功',
        data: {
          updateInfo: 'Pinvou 更新浮窗预览：用于本地检查 UI，不会连接真实 OTA。',
          updateType: 2,
          updateVersion: '9.9.9',
          pkgMd5: '',
          pkgUrl: `http://127.0.0.1:${port}/mock-pinvou-update.zip`,
        },
      });
      return;
    }

    if (req.url === '/ota/pkg/package/updateLog') {
      json(res, { code: 200, success: true, msg: '操作成功', data: {} });
      return;
    }

    if (req.url === '/mock-pinvou-update.zip') {
      res.setHeader('Content-Type', 'application/zip');
      res.end(Buffer.from('mock only'));
      return;
    }

    res.statusCode = 404;
    json(res, { code: 404, success: false, msg: 'not found' });
  });
});

server.listen(port, '127.0.0.1', () => {
  console.log(`mock ota listening http://127.0.0.1:${port}`);
});
