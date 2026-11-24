use super::gui::{Driver, Result};
use serde_json::{Value, json};
use std::path::Path;

fn pixels(driver: &Driver) -> Result<Value> {
    driver.script(
        r#"
        const image=document.querySelector('.source-frame');
        if(!image?.complete||!image.naturalWidth)return null;
        const canvas=document.createElement('canvas');
        canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        return {width:canvas.width,height:canvas.height,
            samples:[[10,10],[20,20],[30,30]].map(([x,y])=>
                Array.from(context.getImageData(x,y,1,1).data).slice(0,3))};
        "#,
    )
}

/// Delays delivery of a real decoded A frame, without holding the native source lock.
/// The ordinary admission path must discard it and drain B without an extra OCR job.
pub fn verify(driver: &Driver, output: &Path) -> Result<()> {
    driver.click("nav[aria-label='Workspace navigation'] .nav-item:nth-of-type(1)")?;
    driver.wait("return document.querySelector('h1')?.textContent==='Extract' && document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='false';", 5)?;
    let expected = pixels(driver)?;
    if expected["width"] != 640 || expected["height"] != 360 {
        return Err(
            "Pending-preview witness must begin with the completed English source B".into(),
        );
    }
    let result = (|| -> Result<Value> {
        driver.script(
        r#"
        const tauri=window.__TAURI__,core=tauri.core;
        if(Object.getOwnPropertyDescriptor(tauri,'core')?.writable!==true)
            throw new Error('The Tauri core namespace cannot be temporarily replaced');
        const audit={core,invoke:core.invoke,create:URL.createObjectURL,revoke:URL.revokeObjectURL,
            requests:[],urls:new Map(),active:0,maxActive:0,returning:null,
            armed:false,held:null,heldReady:false,released:false,release:null,lateAssignments:0,closed:false};
        window.__nvsfPendingPreviewAudit=audit;
        const inspect=url=>{
            const record=audit.urls.get(url);
            if(record&&audit.held!==null&&record.request===audit.held)audit.lateAssignments++;
        };
        const observer=new MutationObserver(records=>{
            for(const record of records){
                if(record.type==='attributes'){
                    inspect(record.oldValue);inspect(record.target.getAttribute('src'));
                }
                for(const node of record.addedNodes){
                    if(node.nodeType!==Node.ELEMENT_NODE)continue;
                    if(node.matches('.source-frame'))inspect(node.getAttribute('src'));
                    for(const image of node.querySelectorAll('.source-frame'))inspect(image.getAttribute('src'));
                }
            }
        });
        observer.observe(document.body,{subtree:true,childList:true,attributes:true,
            attributeFilter:['src'],attributeOldValue:true});
        audit.observer=observer;
        URL.createObjectURL=function(blob){
            const url=audit.create.apply(this,arguments);
            audit.urls.set(url,{request:audit.returning,revoked:0});return url;
        };
        URL.revokeObjectURL=function(url){
            const record=audit.urls.get(url);if(record)record.revoked++;
            return audit.revoke.apply(this,arguments);
        };
        const invoke=function(command,args){
            if(command!=='preview_transport')return audit.invoke.apply(this,arguments);
            const record={id:audit.requests.length,at:args.timeMs,step:args.step,
                beforeRelease:audit.heldReady&&!audit.released};
            audit.requests.push(record);audit.active++;
            audit.maxActive=Math.max(audit.maxActive,audit.active);
            const hold=audit.armed&&audit.held===null&&args.timeMs===100&&args.step===0;
            if(hold){audit.armed=false;audit.held=record.id;}
            let pending;
            try{pending=audit.invoke.apply(this,arguments);}
            catch(error){audit.active--;throw error;}
            return pending.then(value=>{
                record.nativeReady=true;
                if(hold&&!audit.closed)return new Promise(resolve=>{
                    audit.heldReady=true;
                    audit.release=()=>{
                        if(audit.released)return;
                        audit.released=true;audit.returning=record.id;resolve(value);
                    };
                });
                audit.returning=record.id;return value;
            }).finally(()=>audit.active--);
        };
        // Tauri freezes core.invoke, but its public core namespace is replaceable.
        tauri.core=Object.freeze({...core,invoke});
        if(tauri.core.invoke!==invoke)throw new Error('Preview response interceptor was not installed');
        return true;
        "#,
    )?;
        driver.click("[data-testid=e2e-load-contrast]")?;
        driver.wait("const image=document.querySelector('.source-frame');return image?.complete&&image.naturalWidth===320&&image.naturalHeight===240&&document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='false'&&document.querySelector('[data-testid=e2e-load-contrast]')?.disabled===false;", 15)?;
        let before = pixels(driver)?;
        if before["samples"].as_array().is_none_or(|samples| {
            samples.iter().any(|sample| {
                sample[0].as_u64().unwrap_or(0) < 150 || sample[1].as_u64().unwrap_or(255) > 80
            })
        }) {
            return Err(
                format!("Pending source A did not show its real red pixels: {before}").into(),
            );
        }
        driver.script("const audit=window.__nvsfPendingPreviewAudit,input=document.querySelector('#preview-timeline');audit.armed=true;input.value='100';input.dispatchEvent(new Event('input',{bubbles:true}));return true;")?;
        driver.wait("return window.__nvsfPendingPreviewAudit.heldReady===true && document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='true';", 15)?;
        driver.click("[data-testid=e2e-load-queue-fixture]")?;
        driver.wait("return document.querySelector('.source-line strong')?.textContent==='en.mp4' && document.querySelector('[data-testid=e2e-load-queue-fixture]')?.disabled===false && !document.querySelector('.source-frame') && document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='true';", 10)?;
        let held = driver.script("const a=window.__nvsfPendingPreviewAudit;return {heldReady:a.heldReady,released:a.released,active:a.active,maxActive:a.maxActive,bStartedTooSoon:a.requests.some(r=>r.beforeRelease),lateAssignments:a.lateAssignments,retired:Array.from(a.urls.values()).every(r=>r.revoked===1)};")?;
        if held["released"] != false
            || held["active"] != 1
            || held["maxActive"] != 1
            || held["bStartedTooSoon"] != false
            || held["lateAssignments"] != 0
            || held["retired"] != true
        {
            return Err(
                format!("New source admission lost pending preview ownership: {held}").into(),
            );
        }
        driver.script("window.__nvsfPendingPreviewAudit.release();return true;")?;
        driver.wait("const a=window.__nvsfPendingPreviewAudit,image=document.querySelector('.source-frame');return a.active===0&&image?.complete&&image.naturalWidth===640&&image.naturalHeight===360&&document.querySelector('.preview-shell')?.getAttribute('aria-busy')==='false';", 15)?;
        let actual = pixels(driver)?;
        let expected_samples = expected["samples"]
            .as_array()
            .ok_or("Missing source B samples")?;
        let actual_samples = actual["samples"]
            .as_array()
            .ok_or("Missing drained source B samples")?;
        if expected_samples.len() != actual_samples.len()
            || expected_samples.iter().zip(actual_samples).any(|(a, b)| {
                (0..3).any(|index| {
                    a[index]
                        .as_i64()
                        .zip(b[index].as_i64())
                        .is_none_or(|(a, b)| (a - b).abs() > 3)
                })
            })
        {
            return Err(
                format!("Drained preview did not show actual source B pixels: {actual}").into(),
            );
        }
        let completed = driver.script(
            r#"
            const a=window.__nvsfPendingPreviewAudit,current=document.querySelector('.source-frame')?.src;
            const urls=Array.from(a.urls,([url,record])=>({request:record.request,
                revoked:record.revoked,current:url===current,held:record.request===a.held}));
            return {requests:a.requests,active:a.active,maxActive:a.maxActive,
                lateAssignments:a.lateAssignments,urls,
                displayedTime:document.querySelector('.preview-shell')?.dataset.displayedTimeMs};
            "#,
        )?;
        let urls = completed["urls"]
            .as_array()
            .ok_or("Missing preview URL ownership receipt")?;
        if completed["active"] != 0
            || completed["maxActive"] != 1
            || completed["lateAssignments"] != 0
            || completed["displayedTime"] != "0"
            || urls.len() != 3
            || urls.iter().filter(|url| url["current"] == true).count() != 1
            || urls.iter().filter(|url| url["held"] == true).count() != 1
            || urls.iter().any(|url| {
                let expected_revocations = if url["current"] == true { 0 } else { 1 };
                url["revoked"] != expected_revocations
                    || (url["held"] == true && url["current"] != false)
            })
        {
            return Err(
                format!("Late preview publication or URL retirement failed: {completed}").into(),
            );
        }
        Ok(json!({
            "source_sha":std::env::var("GITHUB_SHA")?,
            "target":std::env::var("NVSF_TARGET_ID")?,
            "before_a":before,"expected_b":expected,"admitted_while_a_pending":held,
            "completed_b":actual,"ownership":completed,"passed":true,
            "proof":["real A response delayed after native decode","old image removed before delivery",
                "one frontend preview owner","late A URL never assigned and revoked once",
                "B decoded and displayed without manual seek"]
        }))
    })();
    let restored = driver.script(
        r#"
        const a=window.__nvsfPendingPreviewAudit;
        if(a){
            a.closed=true;
            if(a.release&&!a.released)a.release();
            window.__TAURI__.core=a.core;
            URL.createObjectURL=a.create;URL.revokeObjectURL=a.revoke;
            a.observer?.disconnect();delete window.__nvsfPendingPreviewAudit;
        }
        return true;
        "#,
    );
    let receipt = result?;
    restored?;
    std::fs::write(
        output.join("pending-source-preview.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(())
}
