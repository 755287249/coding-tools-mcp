import { CookieJar, type HttpRequest } from '@coding-tools/plugin-sdk';
import { CODERABBIT, parseSession, type CodeRabbitSession, unwrapTrpc } from './engine.ts';

type Http = (request: HttpRequest) => Promise<{status:number;headers:Record<string,string|string[]>;body:string}>;
export interface SavedAccount { email:string; mailApiUrl:string; password:string; cookies?:unknown; session?:CodeRabbitSession; quota?:number; status?:string }

const sleep=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
export function mailReceivedAt(value:unknown):number { const raw=String(value??'').trim();if(!raw)return NaN;return Date.parse(/[zZ]$|[+-]\d\d:?\d\d$/.test(raw)?raw:raw.replace(' ','T')+'+08:00'); }
function json(text:string):any{try{return JSON.parse(text)}catch{throw Error('Server returned an unexpected response')}}
function location(response:{headers:Record<string,string|string[]>}):string { const value=response.headers.location;return Array.isArray(value)?value[0]??'':value??''; }
function fields(html:string):Record<string,string>{const result:Record<string,string>={};for(const tag of html.match(/<input\b[^>]*>/gi)??[]){const name=tag.match(/\bname=["']([^"']+)/i)?.[1],value=tag.match(/\bvalue=["']([^"']*)/i)?.[1]??'';if(name)result[name]=value.replace(/&amp;/g,'&');}return result}
async function request(jar:CookieJar,http:Http,input:HttpRequest){const cookie=jar.header(input.url),response=await http({...input,headers:{...input.headers,...(cookie?{cookie}:{})}});jar.set(input.url,response.headers['set-cookie']);return response}
async function trpc(jar:CookieJar,http:Http,procedure:string,input:unknown,token=''){const response=await request(jar,http,{url:`${CODERABBIT}/trpc/${procedure}?batch=1`,method:'POST',headers:{'content-type':'application/json','x-trpc-source':'react',...(token?{authorization:`Bearer ${token}`}:{})},body:JSON.stringify({0:input})});if(response.status>=400)throw Error(`CodeRabbit ${procedure} failed (HTTP ${response.status})`);return unwrapTrpc(json(response.body))}

async function begin(jar:CookieJar,http:Http){return trpc(jar,http,'accessToken.beginBrowserOAuth',{provider:'github',redirectUri:'https://app.coderabbit.ai/login',sessionIntent:'sign_in'})}
async function authorize(jar:CookieJar,http:Http,state:string){return request(jar,http,{url:`https://github.com/login/oauth/authorize?client_id=45872a44c0e55c462eed&scope=${encodeURIComponent('user:email read:org')}&state=${encodeURIComponent(state)}&redirect_uri=${encodeURIComponent('https://app.coderabbit.ai/login')}`,method:'GET',headers:{accept:'text/html'}})}
function oauthCode(target:string):string { try{return new URL(target,'https://github.com').searchParams.get('code')??''}catch{return''} }

async function finish(jar:CookieJar,http:Http,state:string,code:string,email:string):Promise<CodeRabbitSession>{
  const exchanged=await trpc(jar,http,'accessToken.exchangeBrowserOAuthCode',{state,code,isCoderabbitSelfHosted:false});
  let access=String(exchanged?.data?.accessToken??exchanged?.accessToken??'');if(!access)throw Error('CodeRabbit OAuth exchange did not return a token');
  const userResult=await trpc(jar,http,'session.getUser',{},access);access=String(userResult?.rotatedAccessToken??access);
  const user=userResult?.data?.user??userResult?.user??userResult?.data,org=userResult?.data?.currentOrganization??userResult?.currentOrganization;
  if(!user?.id||!org?.id)throw Error('CodeRabbit session response changed');
  return parseSession({accessToken:access,user:JSON.stringify({state:{user,currentOrganization:org}}),clerkGitProvider:'github',clerkSsoWorkspaceId:String(org.workspace_id??'')}) as CodeRabbitSession & {cookies:string};
}

/** Pure-protocol GitHub + CodeRabbit OAuth, including verified-device mail OTP. */
export async function login(account:SavedAccount,http:Http,onStatus:(value:string)=>void,signal?:AbortSignal):Promise<SavedAccount>{
  const jar=CookieJar.fromJSON(account.cookies),started=Date.now();onStatus('Starting CodeRabbit OAuth…');
  const oauth=await begin(jar,http),state=String(oauth?.state??'');if(!state)throw Error('CodeRabbit did not return OAuth state');
  let response=await authorize(jar,http,state),next=location(response);
  if(oauthCode(next)){const session=await finish(jar,http,state,oauthCode(next),account.email);session.cookies=jar.header(CODERABBIT);return {...account,cookies:jar.toJSON(),session,status:'Logged in'}}
  const loginPage=await request(jar,http,{url:new URL(next||'/login','https://github.com').toString(),method:'GET',headers:{accept:'text/html'}}),form=fields(loginPage.body);
  onStatus('Signing in to GitHub…');
  response=await request(jar,http,{url:'https://github.com/session',method:'POST',headers:{'content-type':'application/x-www-form-urlencoded','accept':'text/html'},body:new URLSearchParams({...form,login:account.email,password:account.password}).toString()});next=location(response);
  if(next.includes('/sessions/verified-device')){
    const verifyPage=await request(jar,http,{url:new URL(next,'https://github.com').toString(),method:'GET',headers:{accept:'text/html'}}),verify=fields(verifyPage.body);onStatus('Waiting for the GitHub verification email…');
    let otp='';const deadline=Date.now()+10*60_000;
    while(Date.now()<deadline&&!otp){if(signal?.aborted)throw Error('Login stopped');const mail=await http({url:account.mailApiUrl,method:'GET',headers:{accept:'application/json'}});if(mail.status===200){const value=json(mail.body),received=mailReceivedAt(value.received_at);if(value.success&&value.code&&Number.isFinite(received)&&received>=started-30_000)otp=String(value.code).replace(/\D/g,'');}if(!otp)await sleep(10_000)}
    if(!otp)throw Error('Timed out waiting for the GitHub verification email');
    response=await request(jar,http,{url:'https://github.com/sessions/verified-device',method:'POST',headers:{'content-type':'application/x-www-form-urlencoded','accept':'text/html'},body:new URLSearchParams({...verify,otp}).toString()});next=location(response);
  }
  response=await authorize(jar,http,state);next=location(response);const code=oauthCode(next);if(!code)throw Error('GitHub did not complete CodeRabbit authorization');
  const session=await finish(jar,http,state,code,account.email);session.cookies=jar.header(CODERABBIT);return {...account,cookies:jar.toJSON(),session,status:'Logged in'};
}

export async function quota(account:SavedAccount,http:Http):Promise<number>{if(!account.session)throw Error('Log in first');const jar=CookieJar.fromJSON(account.cookies),data=await trpc(jar,http,'onDemandProducts.access',{},account.session.access);const agent=data?.agent??{},seconds=(Number(agent.remainingSeconds)||0)+(Number(agent.monthlyRemainingSeconds)||0);return Math.max(0,Math.floor(seconds/60))}
