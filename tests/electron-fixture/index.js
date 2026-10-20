// Offline fields on the same Electron runtime as the installed application.
const {app, BrowserWindow} = require('electron');
app.whenReady().then(() => {
  const window = new BrowserWindow({width: 700, height: 400});
  const field = process.env.PUNTO_TEST_RICH
    ? '<div contenteditable="true" role="textbox" id="field" style="height:200px;white-space:pre-wrap"></div>'
    : '<textarea id="field" style="width:90%;height:200px"></textarea>';
  window.loadURL('data:text/html;charset=utf-8,' + encodeURIComponent(
    '<title>Punto Electron Test</title>' + field + '<input type="password" id="secret">'));
  window.webContents.once('did-finish-load', () => {
    window.webContents.executeJavaScript(`
      const field = document.getElementById('field');
      if (field.tagName === 'TEXTAREA') {
        field.value = '😀 ';
        field.focus(); field.setSelectionRange(field.value.length, field.value.length);
      } else {
        field.textContent = '😀 '; field.focus();
        const range = document.createRange(); range.selectNodeContents(field); range.collapse(false);
        const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
      }
    `);
  });
});

// Test-only observation without accessibility, clipboard access, or injected keyboard events.
if (process.env.PUNTO_TEST_OUTPUT) {
  const fs = require('fs');
  app.whenReady().then(() => setInterval(async () => {
    const window = BrowserWindow.getAllWindows()[0];
    if (!window || window.isDestroyed()) return;
    try {
      const value = await window.webContents.executeJavaScript(`JSON.stringify({
        text: document.getElementById('field').value ?? document.getElementById('field').textContent,
        password: document.getElementById('secret').value
      })`);
      fs.writeFileSync(process.env.PUNTO_TEST_OUTPUT + '.tmp', value);
      fs.renameSync(process.env.PUNTO_TEST_OUTPUT + '.tmp', process.env.PUNTO_TEST_OUTPUT);
    } catch (_) {}
  }, 20));
}
