import {KnownNodeId} from '../knownids.js';

class NodeData {
  id: string;
  content_type: string;
  content: string;
  constructor(id: string, content_type: string, content: string) {
    this.id = id;
    this.content_type = content_type;
    this.content = content;
  }

}

export function GetNodeDataAndPrintToJQueryNode(id: string, jn: JQuery) {
	console.log(KnownNodeId.kStringEncodedAsURIComponent);
  $.get(`/uniqnode/v0/node/${id}`, (data: any) => {
    var n = new NodeData(data.id, data.content_type, data.content);
    console.log(n);
    if(data.content_type === KnownNodeId.kStringEncodedAsURIComponent){
      jn.text(decodeURIComponent(n.content));
    }
  });
}
