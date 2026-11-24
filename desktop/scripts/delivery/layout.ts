// A browser-run hit test checks real Delivery controls after ordinary scrolling in the smallest window.
/** CSS text cannot prove reachability; each enabled control must actually receive its center hit. */
export function verifyDeliveryControls(page:Document):string {
  const panel=page.querySelector<HTMLElement>(".delivery-panel"),controls=[...(panel?.querySelectorAll<HTMLElement>("button:not(:disabled),input:not(:disabled),textarea:not(:disabled),select:not(:disabled)")??[])];
  if(!panel||!controls.length)return "FAIL: Delivery has no enabled controls";
  const scroll=panel.scrollTop,missed:string[]=[];
  for(const control of controls) {
    if(control.closest("details:not([open])"))continue;
    control.scrollIntoView({block:"center",inline:"nearest",behavior:"instant"});
    const rect=control.getBoundingClientRect(),hit=page.elementFromPoint(rect.left+rect.width/2,rect.top+rect.height/2);
    if(!rect.width||!rect.height||!hit||!control.contains(hit))missed.push(control.getAttribute("aria-label")||control.textContent?.trim()||control.tagName);
  }
  panel.scrollTop=scroll;
  return missed.length?`FAIL: ${missed.length} Delivery controls unreachable: ${missed.join(", ")}`:`PASS: Delivery controls reachable by scrolling`;
}
