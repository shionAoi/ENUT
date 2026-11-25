import type {Agent} from './types.ts';

// The caller has captured consent, including Gemini's explicit native login.
export async function connectIntegration(
 tool:Agent['id'],
 openNativeLogin:(tool:string,onFinished:()=>Promise<void>)=>Promise<void>,
 grantMonitoring:()=>Promise<void>,
):Promise<void>{
 if(tool==='antigravity')await openNativeLogin(tool,grantMonitoring);
 else await grantMonitoring();
}
