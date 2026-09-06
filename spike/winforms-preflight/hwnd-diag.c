/* Is the tree missing, or is it hanging off the child control HWNDs? */
#define COBJMACROS
#include <windows.h>
#include <oleacc.h>
#include <stdio.h>

static HWND top;
static BOOL CALLBACK pick(HWND h, LPARAM lp){
    char t[256]={0}; GetWindowTextA(h,t,sizeof t);
    if(strstr(t,(const char*)lp)){ top=h; return FALSE; } return TRUE;
}
static BOOL CALLBACK kid(HWND h, LPARAM depth){
    char cls[128]={0}, txt[128]={0};
    GetClassNameA(h,cls,sizeof cls); GetWindowTextA(h,txt,sizeof txt);
    IAccessible *acc=NULL; char rbuf[128]="<none>"; long role=0, n=-1;
    HRESULT hr=AccessibleObjectFromWindow(h,OBJID_CLIENT,&IID_IAccessible,(void**)&acc);
    if(SUCCEEDED(hr)&&acc){
        VARIANT self,vr; VariantInit(&vr);
        self.vt=VT_I4; self.lVal=CHILDID_SELF;
        if(SUCCEEDED(IAccessible_get_accRole(acc,self,&vr))&&vr.vt==VT_I4) role=vr.lVal;
        if(!GetRoleTextA((DWORD)role,rbuf,sizeof rbuf)) snprintf(rbuf,sizeof rbuf,"<%ld>",role);
        IAccessible_get_accChildCount(acc,&n);
        BSTR nm=NULL;
        if(SUCCEEDED(IAccessible_get_accName(acc,self,&nm))&&nm){
            printf("      accName=\"%ls\"\n", nm); SysFreeString(nm);
        } else printf("      accName=<none>\n");
        IAccessible_Release(acc);
    } else snprintf(rbuf,sizeof rbuf,"HR=0x%08lx",(unsigned long)hr);
    printf("  %-22s txt=%-14s role=%-20s children=%ld\n",cls,txt,rbuf,n);
    (void)depth; return TRUE;
}
int main(int argc,char**argv){
    const char*want=argc>1?argv[1]:"PreflightTarget";
    CoInitialize(NULL);
    for(int i=0;i<40&&!top;i++){ EnumWindows(pick,(LPARAM)want); if(!top) Sleep(500);}
    if(!top){fprintf(stderr,"not found\n");return 2;}
    printf("top-level %p\n", (void*)top);
    printf("child HWNDs:\n");
    EnumChildWindows(top,kid,0);
    CoUninitialize(); return 0;
}
