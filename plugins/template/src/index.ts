import { plugin } from '@coding-tools/plugin-sdk';

document.body.innerHTML = `<main><h1>Hello plugin</h1><p>This iframe is sandboxed. Host features are available through the SDK.</p><button>Save click</button><output></output></main>`;
const button = document.querySelector('button')!, output = document.querySelector('output')!;
button.addEventListener('click', async () => { const data = await plugin.storage.get<{ clicks?: number }>(); const clicks = (data.clicks ?? 0) + 1; await plugin.storage.set({ clicks }); output.textContent = `Saved clicks: ${clicks}`; });
void plugin.ready();
