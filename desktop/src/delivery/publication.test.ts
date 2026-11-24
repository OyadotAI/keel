// Publication status follows its exact durable receipt after retrying from another surface.
import {expect,it,vi} from "vitest";
vi.mock("./state",()=>({refreshDelivery:vi.fn()}));
import {publicationReceipt} from "./publication";
const stale={id:"captured",kind:"publication",phase:"unconfirmed",error:"Lost response"};
it("replaces an unconfirmed local label after its saved-operation retry completes",()=>{expect(publicationReceipt(stale,[{id:"captured",kind:"publication",phase:"completed"}])).toEqual({id:"captured",kind:"publication",phase:"completed"});});
it("cannot use another receipt or an older projection to regress a confirmed result",()=>{expect(publicationReceipt(stale,[{id:"other",kind:"publication",phase:"completed"}])).toBe(stale);const completed={...stale,phase:"completed"};expect(publicationReceipt(completed,[stale])).toBe(completed);});
