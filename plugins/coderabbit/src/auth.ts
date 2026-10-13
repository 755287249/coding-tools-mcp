import { CookieJar, type HttpRequest } from '@coding-tools/plugin-sdk';
import { CODERABBIT, parseSession, type CodeRabbitSession, unwrapTrpc } from './engine.ts';

type Http = (request: HttpRequest) => Promise<{status:number;headers:Record<string,string|string[]>;body:string}>;
export interface SavedAccount { email:string; mailApiUrl:string; password:string; cookies?:unknown; session?:CodeRabbitSession; quota?:number; status?:string }

const sleep=(ms:number)=>new Promise(resolve=>setTimeout(resolve,ms));
export function mailReceivedAt(value:unknown):number { const raw=String(value??'').trim();if(!raw)return NaN;return Date.parse(/[zZ]$|[+-]\d\d:?\d\d$/.test(raw)?raw:raw.replace(' ','T')+'+08:00'); }
function json(text:string):any{try{return JSON.parse(text)}catch{throw Error('Server returned an unexpected response')}}
function location(response:{headers:Record<string,string|string[]>}):string { const value=response.headers.location;return Array.isArray(value)?value[0]??'':value??''; }
/** Input values of one form. GitHub pages contain several forms (e.g. verified-device + resend), each with its own
 * authenticity_token, so inputs must be read from the form being submitted or GitHub answers HTTP 422. */
export function fields(html:string,action?:string):Record<string,string>{
  let scope=html;
  if(action){const forms=html.match(/<form\b[\s\S]*?<\/form>/gi)??[];scope=forms.find(form=>{const target=form.match(/\baction=["']([^"']*)/i)?.[1]?.replace(/&amp;/g,'&')??'';return target.split('?')[0]===action})??''}
  const result:Record<string,string>={};for(const tag of scope.match(/<input\b[^>]*>/gi)??[]){const name=tag.match(/\bname=["']([^"']+)/i)?.[1],value=tag.match(/\bvalue=["']([^"']*)/i)?.[1]??'';if(name&&!(name in result))result[name]=value.replace(/&amp;/g,'&');}return result}
function pageError(html:string):string{const text=html.match(/<div[^>]*class="[^"]*flash-error[^"]*"[^>]*>([\s\S]*?)<\/div>/i)?.[1]??'';return text.replace(/<[^>]+>/g,' ').replace(/\s+/g,' ').trim().slice(0,200)}
async function latestMail(http:Http,url:string):Promise<{id:string;code:string;received:number}>{const mail=await http({url,method:'GET',headers:{accept:'application/json'}});if(mail.status!==200)return {id:'',code:'',received:NaN};let value:any;try{value=JSON.parse(mail.body)}catch{return {id:'',code:'',received:NaN}}return {id:String(value?.message_id??''),code:value?.success?String(value.code??'').replace(/\D/g,''):'',received:mailReceivedAt(value?.received_at)}}
async function request(jar:CookieJar,http:Http,input:HttpRequest){const cookie=jar.header(input.url),response=await http({...input,headers:{...input.headers,...(cookie?{cookie}:{})}});jar.set(input.url,response.headers['set-cookie']);return response}
// tRPC queries must use GET (?input=); mutations use POST. A query sent as POST answers HTTP 405.
const QUERIES=new Set(['session.getUser','onDemandProducts.access','codingAgent.personalTrial']);
async function trpc(jar:CookieJar,http:Http,procedure:string,input:unknown,token='',extra:Record<string,string>={}){const get=QUERIES.has(procedure);const response=await request(jar,http,{url:`${CODERABBIT}/trpc/${procedure}?batch=1${get?'&input='+encodeURIComponent(JSON.stringify({0:input})):''}`,method:get?'GET':'POST',headers:{'content-type':'application/json','x-trpc-source':'react',...(token?{authorization:`Bearer ${token}`}:{}),...extra},...(get?{}:{body:JSON.stringify({0:input})})});if(response.status>=400)throw Error(`CodeRabbit ${procedure} failed (HTTP ${response.status})`);return unwrapTrpc(json(response.body))}

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
  const loginPage=await request(jar,http,{url:new URL(next||'/login','https://github.com').toString(),method:'GET',headers:{accept:'text/html'}}),form=fields(loginPage.body,'/session');
  if(!form.authenticity_token)throw Error('GitHub login page changed (no sign-in form)');
  // Remember the newest mail before signing in so an older verification code is never reused.
  const before=await latestMail(http,account.mailApiUrl).catch(()=>({id:'',code:'',received:NaN}));
  onStatus('Signing in to GitHub…');
  response=await request(jar,http,{url:'https://github.com/session',method:'POST',headers:{'content-type':'application/x-www-form-urlencoded','accept':'text/html'},body:new URLSearchParams({...form,login:account.email,password:account.password}).toString()});next=location(response);
  if(response.status===200||/\/(session|login)(\?|$)/.test(new URL(next||'/session','https://github.com').pathname+'?'))throw Error(`GitHub sign-in failed: ${pageError(response.body)||'check the password'}`);
  if(next.includes('/sessions/two-factor'))throw Error('This GitHub account uses two-factor authentication, which is not supported');
  if(next.includes('/sessions/verified-device')){
    let tries=0;
    while(true){
      const verifyPage=await request(jar,http,{url:'https://github.com/sessions/verified-device',method:'GET',headers:{accept:'text/html'}}),verify=fields(verifyPage.body,'/sessions/verified-device');
      if(!verify.authenticity_token){next=location(verifyPage);break}
      onStatus('Waiting for the GitHub verification email…');
      let otp='';const deadline=Date.now()+10*60_000;
      while(Date.now()<deadline&&!otp){if(signal?.aborted)throw Error('Login stopped');const mail=await latestMail(http,account.mailApiUrl);if(mail.code&&mail.id!==before.id&&(!Number.isFinite(mail.received)||mail.received>=started-120_000))otp=mail.code;if(!otp)await sleep(5_000)}
      if(!otp)throw Error('Timed out waiting for the GitHub verification email');
      onStatus('Submitting the verification code…');
      response=await request(jar,http,{url:'https://github.com/sessions/verified-device',method:'POST',headers:{'content-type':'application/x-www-form-urlencoded','accept':'text/html'},body:new URLSearchParams({...verify,otp}).toString()});next=location(response);
      if(response.status>=300&&response.status<400&&!next.includes('/sessions/verified-device'))break;
      before.id=(await latestMail(http,account.mailApiUrl).catch(()=>before)).id;
      if(++tries>=2)throw Error(`GitHub rejected the verification code (HTTP ${response.status}) ${pageError(response.body)}`.trim());
    }
  }
  onStatus('Authorizing CodeRabbit…');
  response=await authorize(jar,http,state);next=location(response);
  // First login of an account shows GitHub's "Authorize CodeRabbit" consent page instead of redirecting.
  if(!oauthCode(next)&&response.status===200){const consent=fields(response.body,'/login/oauth/authorize');if(consent.authenticity_token){response=await request(jar,http,{url:'https://github.com/login/oauth/authorize',method:'POST',headers:{'content-type':'application/x-www-form-urlencoded','accept':'text/html'},body:new URLSearchParams({...consent,authorize:'1'}).toString()});next=location(response)}}
  const code=oauthCode(next);if(!code)throw Error(`GitHub did not complete CodeRabbit authorization (HTTP ${response.status}${next?' → '+new URL(next,'https://github.com').pathname:''})`);
  const session=await finish(jar,http,state,code,account.email);session.cookies=jar.header(CODERABBIT);return {...account,cookies:jar.toJSON(),session,status:'Logged in'};
}

export async function quota(account:SavedAccount,http:Http):Promise<number>{if(!account.session)throw Error('Log in first');const jar=CookieJar.fromJSON(account.cookies),data=await trpc(jar,http,'onDemandProducts.access',{},account.session.access,{'x-clerk-git-provider':account.session.provider||'github','x-coderabbitai-organization':account.session.organization,...(account.session.workspace?{'x-coderabbitai-workspace':account.session.workspace}:{})});const agent=data?.agent??{},seconds=(Number(agent.remainingSeconds)||0)+(Number(agent.monthlyRemainingSeconds)||0);return Math.max(0,Math.floor(seconds/60))}

/** CodeRabbit tRPC needs its session cookies besides the Bearer token; keep the account jar in sync. */
export function cookieHttp(account:SavedAccount,http:Http):Http{const jar=CookieJar.fromJSON(account.cookies);return async input=>{const response=await request(jar,http,input);account.cookies=jar.toJSON();return response}}
/** Smallest positive quota first; accounts with 0 or unknown quota go last. */
export function sortAccounts<T extends {quota?:number}>(accounts:T[]):T[]{const rank=(a:T)=>(a.quota??0)>0?a.quota!:Number.POSITIVE_INFINITY;return [...accounts].sort((a,b)=>rank(a)-rank(b))}
