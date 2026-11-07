/** 用户管理对话框：新建用户 / 修改密码（MySQL 协议族）。 */

import { api } from "../core/api";
import { Dialog, field, passwordInput, textInput } from "../ui/Dialog";

/** 新建用户。 */
export function openCreateUserDialog(
  connId: string,
  onSaved: () => void,
): Dialog {
  const userInput = textInput("", 200);
  const hostInput = textInput("%", 200);
  const passInput = passwordInput("", 200);
  const pass2Input = passwordInput("", 200);
  const dlg = new Dialog({
    title: "新建用户",
    width: 420,
    render: (body) => {
      body.appendChild(field("用户名:", userInput));
      body.appendChild(field("主机:", hostInput));
      const note = document.createElement("div");
      note.style.cssText = "font-size:11px;color:#8A929E;margin:-2px 0 6px 110px";
      note.textContent = "允许连接的主机，% 表示任意主机";
      body.appendChild(note);
      body.appendChild(field("密码:", passInput));
      body.appendChild(field("确认密码:", pass2Input));
    },
    buttons: [
      {
        label: "创建",
        primary: true,
        onClick: async () => {
          const user = userInput.value.trim();
          if (!user) {
            dlg.showError("请填写用户名");
            return false;
          }
          if (passInput.value !== pass2Input.value) {
            dlg.showError("两次输入的密码不一致");
            return false;
          }
          try {
            await api.createDbUser(connId, user, hostInput.value.trim(), passInput.value);
            onSaved();
          } catch (e) {
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "取消" },
    ],
  });
  return dlg;
}

/** 修改用户密码。 */
export function openChangePasswordDialog(
  connId: string,
  user: string,
  host: string,
  onSaved: () => void,
): Dialog {
  const passInput = passwordInput("", 200);
  const pass2Input = passwordInput("", 200);
  const dlg = new Dialog({
    title: `修改密码（${user}@${host}）`,
    width: 400,
    render: (body) => {
      body.appendChild(field("新密码:", passInput));
      body.appendChild(field("确认密码:", pass2Input));
    },
    buttons: [
      {
        label: "确定",
        primary: true,
        onClick: async () => {
          if (passInput.value !== pass2Input.value) {
            dlg.showError("两次输入的密码不一致");
            return false;
          }
          try {
            await api.alterDbUserPassword(connId, user, host, passInput.value);
            onSaved();
          } catch (e) {
            dlg.showError(String(e));
            return false;
          }
          return true;
        },
      },
      { label: "取消" },
    ],
  });
  return dlg;
}
