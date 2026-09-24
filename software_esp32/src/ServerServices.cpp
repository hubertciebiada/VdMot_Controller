/**HEADER*******************************************************************
  project : VdMot Controller

  author : SurfGargano, Lenti84

  Comments:

  Version :

  Modifcations :


***************************************************************************
*
* THIS SOFTWARE IS PROVIDED "AS IS" AND ANY EXPRESSED OR
* IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
* OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
* IN NO EVENT SHALL THE DEVELOPER OR ANY CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
* INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
* (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
* SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
* HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
* STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
* IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF
* THE POSSIBILITY OF SUCH DAMAGE.
*
**************************************************************************
  This program is free software: you can redistribute it and/or modify
  it under the terms of the GNU General Public License as published by
  the Free Software Foundation, either version 3 of the License.
  See the GNU General Public License for more details.

  You should have received a copy of the GNU General Public License
  along with this program.  If not, see <http://www.gnu.org/licenses/>.

  Copyright (C) 2021 Lenti84  https://github.com/Lenti84/VdMot_Controller

*END************************************************************************/


#include <stdint.h>
#include <float.h>
#include <math.h>
#include <VdmNet.h>
#include <WiFi.h>
#include <WiFiClient.h>

#include "globals.h"
#include "WT32AsyncOTA.h"

#include "web.h"
#include "tfs.h"

#include "time.h"

#include "stm32.h"
#include "stm32ota.h"
#include <ESPmDNS.h>
#include <WiFiUdp.h>
#include "ServerServices.h"
#include "helper.h"
#include "VdmSystem.h"
#include "VdmTask.h"
#include "Services.h"
#include "stmApp.h"
#include "PIControl.h"
#include "Messenger.h"
#include "mqtt.h"

#include <FS.h>
#ifdef USE_LittleFS
  #define SPIFFS LittleFS
  #include <LittleFS.h> 
#else
  #include <SPIFFS.h>
#endif 

extern "C" {
  #include "tfs_data.h"
}


#include <AsyncJson.h>
#include <ArduinoJson.h>

AsyncWebServer server(80);

CServerServices ServerServices;

#define aj  "application/json"
#define tp  "text/plain"
#define th  "text/html"
#define gz  "Content-Encoding : gzip"

#define resOk "{\"res\":\"ok\"}"

// server handles --------------------------------------------------


int restart (JsonObject doc)
{  
  // refused while the STM is being flashed, the flasher restarts the ESP when done
  return Services.restartSystem(false) ? 200 : 409;
}

int writeConfig (JsonObject doc)
{  
  VdmConfig.writeConfig(true);
  return 200;
}

int resetConfig (JsonObject doc)
{  
  VdmConfig.resetConfig(true);
  return 200;
}

int restoreConfig (JsonObject doc)
{  
  VdmConfig.restoreConfig(true);
  return 200;
}

int setClearFS (JsonObject doc)
{  
  VdmTask.startClearFS();
  return 200;
}

int setGetFS (JsonObject doc)
{  
  VdmTask.startGetFS();
  return 200;
}

int fileDelete (JsonObject doc)
{  
  if (!doc["file"].isNull()) {
    VdmSystem.fileDelete(doc["file"]);
  }
  return 200;
}

int scanTSensors (JsonObject doc)
{  
  StmApp.scanTemps();
  return 200;
}

// "valve": 1..ACTUATOR_COUNT, or 255 / missing for all valves
bool getValveIndex (JsonObject doc, uint8_t* index)
{
  *index=255;
  if (doc["valve"].isNull()) return true;
  long valve;
  if (!jsonToLong(doc["valve"],&valve)) return false;     // number or numeric string
  if (valve==255) return true;
  if ((valve<1) || (valve>ACTUATOR_COUNT)) return false;
  *index=valve-1;
  return true;
}

int valvesCalibration (JsonObject doc)
{  
  uint8_t index;
  if (!getValveIndex(doc,&index)) return 400;
  StmApp.valvesCalibration(index);
  return 200;
}

int valvesAssembly (JsonObject doc)
{  
  uint8_t index;
  if (!getValveIndex(doc,&index)) return 400;
  StmApp.valvesAssembly(index);
  return 200;
}

int valvesDetect (JsonObject doc)
{  
  StmApp.valvesDetect();
  return 200;
}

int writeValvesControl (JsonObject doc)
{  
  VdmConfig.writeValvesControlConfig(false,VdmTask.restartPiTask);
  Mqtt.forceReconnect=true;
  return 200;
}

int mqttReconnect (JsonObject doc)
{  
  Mqtt.disconnect();
  return 200;
}

int scanWifi (JsonObject doc)
{  
  #ifdef netDebugWIFI
    UART_DBG.println("server servíces cmd : scan wifi "+String(VdmTask.taskIdScanWiFi));
  #endif
  VdmNet.wifiScanState = wifiScanIdle;
  VdmNet.scanRepeatWifi = 0;
  if (VdmTask.taskIdScanWiFi==TASKMGR_INVALIDID) {
      #ifdef netDebugWIFI
            UART_DBG.println("server servíces cmd : Start scan wifi task");
      #endif
      VdmTask.startScanWifi();
  }
  return 200;
}

int sysLogSave (JsonObject doc)
{  
  VdmConfig.writeSysLogValues();
  VdmNet.syslogStarted=false;
  VdmNet.startSysLog();
  return 200;
}

int discoveryHA (JsonObject doc)
{  
  if (Mqtt.hadState==HAD_IDLE) {
    if (!doc["actionHA"].isNull()) 
      Mqtt.actionHA = doc["actionHA"];
    else Mqtt.actionHA = HA_DISCOVERY_ONLY;
    Mqtt.hadState=HAD_STARTED;
  }
  return 200;
}

// Returns false, and changes nothing, when the valve number or any given value
// is invalid: target 0..100 %, ctrlValue/ctrlTarget finite float numbers, ctrlDynOffs
// -128..127. The web UI posts input field values as strings.
bool CServerServices::postSetValve (JsonObject doc)
{
  if (doc["valve"].isNull()) return true;
  long valve;
  if (!jsonToLong(doc["valve"],&valve) || (valve<1) || (valve>ACTUATOR_COUNT)) return false;
  uint8_t index=valve-1;

  bool hasTarget=!doc["value"].isNull();
  bool hasCtrlValue=!doc["ctrlValue"].isNull();
  bool hasCtrlTarget=!doc["ctrlTarget"].isNull();
  bool hasDynOffs=!doc["ctrlDynOffs"].isNull();
  long target=0;
  long dynOffs=0;
  double ctrlValue=0;
  double ctrlTarget=0;
  if (hasTarget && (!jsonToLong(doc["value"],&target) || (target<0) || (target>100))) return false;
  // both are stored as float
  if (hasCtrlValue && (!jsonToDouble(doc["ctrlValue"],&ctrlValue) || (fabs(ctrlValue)>FLT_MAX))) return false;
  if (hasCtrlTarget && (!jsonToDouble(doc["ctrlTarget"],&ctrlTarget) || (fabs(ctrlTarget)>FLT_MAX))) return false;
  if (hasDynOffs && (!jsonToLong(doc["ctrlDynOffs"],&dynOffs) || (dynOffs<INT8_MIN) || (dynOffs>INT8_MAX))) return false;

  if (VdmConfig.configFlash.valvesConfig.valveConfig[index].active) {
    if (hasTarget) StmApp.actuators[index].target_position = target;
  }
  if (VdmConfig.configFlash.valvesControlConfig.valveControlConfig[index].controlFlags.active) {
    if (VdmConfig.configFlash.valvesControlConfig.valveControlConfig[index].valueSource==3) {
      if (hasCtrlValue) PiControl[index].value = ctrlValue;
      #ifdef EnvDevelop
        UART_DBG.println("ctrlValue : "+String(PiControl[index].value));
      #endif
      jsonSetValveReceived=true;
    }
    if (VdmConfig.configFlash.valvesControlConfig.valveControlConfig[index].targetSource==1) {
      if (hasCtrlTarget) {
        PiControl[index].target = ctrlTarget;
        #ifdef EnvDevelop
        UART_DBG.println("ctrlTarget : "+String(PiControl[index].target));
        #endif
        jsonSetValveReceived=true;
      }
      if (hasDynOffs) PiControl[index].dynOffset = dynOffs;
    }
  }
  return true;
}

String getContentType(String filename) 
{ // convert the file extension to the MIME type
  if (filename.endsWith(".html")) return th;
  else if (filename.endsWith(".css")) return "text/css";
  else if (filename.endsWith(".js")) return "application/javascript";
  else if (filename.endsWith(".ico")) return "image/x-icon";
  else if (filename.endsWith(".gz")) return "application/x-gzip";
  return tp;
}

int8_t checkEntry (String url) 
{
  int8_t i = 0;
  TFS_DIR_ENTRY *entry = (TFS_DIR_ENTRY*) tfs_data;
  String thisUrl=url;
  if (url=="/") thisUrl = "/index.html";
 
  while (entry->NAME != NULL) {
      if (thisUrl == String(entry->NAME)) {
        return(i);
      }
      entry++;
      i++;
  }
  
  return(-1);
}

void handleRoot(AsyncWebServerRequest *request) 
{
  int8_t index;
  
  index=checkEntry(request->url());
  if (index>=0) {
    AsyncWebServerResponse *response = request->beginResponse_P(200, getContentType (tfs_data[index].NAME), 
                                      tfs_data[index].DATA, tfs_data[index].SIZE);
    response->addHeader("Content-Encoding", "gzip");
    request->send(response);
  } else {
    String message = "File "+request->url()+" Not Found\n\n"; 
    request->send(404, tp, message);
  }
}


void handleWebPageStmUpdate(AsyncWebServerRequest *request) 
{
  int8_t index;
  String thisUrl = request->url();
  if (!thisUrl.endsWith(".html")) thisUrl+=".html";
  index=checkEntry(thisUrl);
  if (index>=0) {
    AsyncWebServerResponse *response = request->beginResponse_P(200, getContentType (tfs_data[index].NAME), 
                                      tfs_data[index].DATA, tfs_data[index].SIZE);
    response->addHeader("Content-Encoding", "gzip");
    request->send(response);
  } else {
    String message = "File "+request->url()+" Not Found\n\n"; 
    request->send(404, tp, message);
  }
}

void handleNotFound(AsyncWebServerRequest *request)
{
  if (checkEntry(request->url())>=0) {
    handleRoot(request); 
  } else {

    String message = "File Not Found\n\n";

    message += "URI: ";
    //message += server.uri();
    message += request->url();
    message += "\nMethod: ";
    message += (request->method() == HTTP_GET) ? "GET" : "POST";
    message += "\nArguments: ";
    message += request->args();
    message += "\n";

    for (uint8_t i = 0; i < request->args(); i++) {
      message += " " + request->argName(i) + ": " + request->arg(i) + "\n";
    }
    request->send(404, tp, message);
  }
}

void handleValves(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getValvesStatus());
}

void handleTemps(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getTempsStatus(VdmConfig.configFlash.tempsConfig));
}

void handleVolts(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getVoltsStatus(VdmConfig.configFlash.voltsConfig));
}

void handleNetInfo(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getNetInfo(VdmNet.networkInfo));
}

void handleSSIDInfo(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getSSIDInfo());
}

void handleNetConfig(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getNetConfig(VdmConfig.configFlash.netConfig));
}

void handleProtConfig(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getProtConfig(VdmConfig.configFlash.protConfig));
}

void handleValvesConfig(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getValvesConfig (VdmConfig.configFlash.valvesConfig));
}

void handleMotorConfig(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getMotorConfig (StmApp.motorChars));
}

void handleValvesControlConfig(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getValvesControlConfig (VdmConfig.configFlash.valvesControlConfig));
}

void handleTempsConfig(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getTempsConfig (VdmConfig.configFlash.tempsConfig));
}

void handleTempSensorsID(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getTempSensorsID ());
}

void handleVoltsConfig(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getVoltsConfig (VdmConfig.configFlash.voltsConfig));
}

void handleVoltSensorsID(AsyncWebServerRequest *request)
{
  request->send(200,aj,Web.getVoltSensorsID ());
}

void handleSysInfo(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getSysInfo());
}

void handleSysDynInfo(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getSysDynInfo());
}

void handleGetSysConfig(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getSysConfig(VdmConfig.configFlash.systemConfig));
}

void handleGetMsgConfig(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getMsgConfig(VdmConfig.configFlash.messengerConfig));
}

void handleGetStm(AsyncWebServerRequest *request) 
{ 
  // generate callback with request parameter
  // send request answer after getting from stm 
}

void handleGetFSDir(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getFSDir());
}

void handleStmUpdStatus(AsyncWebServerRequest *request) 
{ 
  request->send(200,aj,Web.getStmUpdStatus());
}


// returns the HTTP status of the command, 404 when there is no such command
int handleCmd(JsonObject doc) 
{ 
  typedef int (*fp)(JsonObject doc);
  fp  fpList[] = {&restart,&writeConfig,&resetConfig,&restoreConfig,&fileDelete,&setGetFS,
                  &setClearFS,&scanTSensors,&valvesCalibration,&valvesAssembly,&valvesDetect,&writeValvesControl,&mqttReconnect,&sysLogSave,&discoveryHA,&scanWifi} ;

  char const *names[]=  {"reboot", "saveCfg","resetCfg","restoreCfg","fDelete","getFS",
                        "clearFS","scanTempSensors","vCalib","vAssembly","valvesDetect","vCtrlSave","mqttReconnect","sysLogSave","discoveryHA","scanWifi",NULL};
  char const **p;

  if (doc["action"].is<const char*>()) {      // a non-string action would give NULL
    const char* d=doc["action"].as<const char*>();
    uint8_t i=0;
  
    for (p=names; *p!=NULL; p++) {
      if (strcmp(d, *p)==0) return fpList[i](doc);
      i++;
    }
  }
  return 404;
}

// LittleFS paths must be absolute
String uploadFilePath(const String& filename)
{
  return filename.startsWith("/") ? filename : "/"+filename;
}

// an upload is written to <name>.part and renamed to <name> only when it is
// complete, so an interrupted upload never leaves a truncated file under the
// real name (and never replaces a good file of that name)
static const char uploadTempSuffix[] = ".part";

// The request handler answers from a status byte in _tempObject (freed with the
// request). It is allocated before the first file is opened, so recording a
// failure never needs memory; without it nothing is written and the request
// is answered as failed, as is a request without a file.
enum { uploadOk = 0, uploadFailed = 1 };

bool uploadSucceeded(AsyncWebServerRequest *request)
{
  return (request->_tempObject != NULL) && (*(uint8_t*)request->_tempObject == uploadOk);
}

void markUploadFailed(AsyncWebServerRequest *request)
{
  if (request->_tempObject != NULL) *(uint8_t*)request->_tempObject = uploadFailed;
}

void handleUploadFile(AsyncWebServerRequest *request, const String& filename, size_t index, 
            uint8_t *data, size_t len, bool final)
{
  if(!index){
    String thisFileName = uploadFilePath(filename);
    String tempFileName = thisFileName + uploadTempSuffix;
    // open the file on first call and store the file handle in the request object
    #ifdef EnvDevelop
      UART_DBG.println("file has arguments : "+String(request->args()));
      UART_DBG.println("filename : "+thisFileName);
    #endif
    if (request->_tempObject == NULL) {
      request->_tempObject = malloc(sizeof(uint8_t));
      if (request->_tempObject == NULL) return;    // out of memory: answered as failed
      *(uint8_t*)request->_tempObject = uploadOk;
    }
    if (SPIFFS.exists(tempFileName)) SPIFFS.remove(tempFileName);   // left by an earlier interrupted upload
    request->_tempFile = SPIFFS.open(tempFileName, "w");
    if (!request->_tempFile) markUploadFailed(request);
    else {
      // client gone before the last chunk: drop the partial file
      request->onDisconnect([request, tempFileName]() {
        if (request->_tempFile) {
          request->_tempFile.close();
          SPIFFS.remove(tempFileName);
        }
      });
    }
  }
  if(len && request->_tempFile) {
    // stream the incoming chunk to the opened file
    if (request->_tempFile.write(data,len) != len) {
      // FS full: never leave a truncated (STM firmware) file behind
      request->_tempFile.close();
      SPIFFS.remove(uploadFilePath(filename) + uploadTempSuffix);
      markUploadFailed(request);
    }
  }
  if(final){
    // close the file handle as the upload is now done, then publish it under its name
    if (request->_tempFile) {
      String thisFileName = uploadFilePath(filename);
      String tempFileName = thisFileName + uploadTempSuffix;
      request->_tempFile.close();
      // LittleFS replaces an existing file atomically: the old file stays until
      // the new one is complete. Remove it first only if that did not work.
      bool renamed = SPIFFS.rename(tempFileName, thisFileName);
      if (!renamed && SPIFFS.exists(thisFileName) && SPIFFS.remove(thisFileName))
        renamed = SPIFFS.rename(tempFileName, thisFileName);
      if (!renamed) {
        SPIFFS.remove(tempFileName);
        markUploadFailed(request);
      }
    }
    #ifdef EnvDevelop
      UART_DBG.println("upload finished");
    #endif
  }
  
}

CServerServices::CServerServices()
{
  jsonSetValveReceived=false;
}

// returns the HTTP status: 200 started, 400 bad request, 409 refused
int CServerServices::stmDoUpdate(JsonObject doc)
{
  const char* file = doc["file"].as<const char*>();
  if ((file == NULL) || (*file == '\0') || doc["cmd"].isNull()) return 400;
  String thisFileName = uploadFilePath(file);
  // never flash what is left of an interrupted upload
  if (thisFileName.endsWith(uploadTempSuffix)) return 400;
  uint8_t command = doc["cmd"];
  #ifdef EnvDevelop
    UART_DBG.println("file : "+thisFileName + " Command "+ String(command));
  #endif
  bool started;
  if (command==0) started=VdmTask.startStm32Ota(STM32OTA_START,thisFileName);
  else if (command==1) started=VdmTask.startStm32Ota(STM32OTA_STARTBLANK,thisFileName);
  else return 400;
  return started ? 200 : 409;
}

void  CServerServices::initServer() 
{
  // define on events
  server.on("/", HTTP_GET, [](AsyncWebServerRequest * request) {handleRoot(request);});
  server.onNotFound(handleNotFound);
  server.on("/valves",HTTP_GET,[](AsyncWebServerRequest * request) {handleValves(request);});
  server.on("/temps",HTTP_GET,[](AsyncWebServerRequest * request) {handleTemps(request);});
  server.on("/volts",HTTP_GET,[](AsyncWebServerRequest * request) {handleVolts(request);});
  server.on("/netinfo",HTTP_GET,[](AsyncWebServerRequest * request) {handleNetInfo(request);});
  server.on("/ssidinfo",HTTP_GET,[](AsyncWebServerRequest * request) {handleSSIDInfo(request);});
  server.on("/netconfig",HTTP_GET,[](AsyncWebServerRequest * request) {handleNetConfig(request);});
  server.on("/protconfig",HTTP_GET,[](AsyncWebServerRequest * request) {handleProtConfig(request);});
  server.on("/valvesconfig",HTTP_GET,[](AsyncWebServerRequest * request) {handleValvesConfig(request);});
  server.on("/motorconfig",HTTP_GET,[](AsyncWebServerRequest * request) {handleMotorConfig(request);});
  server.on("/valvesctrlconfig",HTTP_GET,[](AsyncWebServerRequest * request) {handleValvesControlConfig(request);});
  server.on("/tempsconfig",HTTP_GET,[](AsyncWebServerRequest * request) {handleTempsConfig(request);});
  server.on("/voltsconfig",HTTP_GET,[](AsyncWebServerRequest * request) {handleVoltsConfig(request);});
  server.on("/sysinfo",HTTP_GET,[](AsyncWebServerRequest * request) {handleSysInfo(request);});
  server.on("/sysdyninfo",HTTP_GET,[](AsyncWebServerRequest * request) {handleSysDynInfo(request);});
  server.on("/fsdir",HTTP_GET,[](AsyncWebServerRequest * request) {handleGetFSDir(request);});
  server.on("/stmupdate", HTTP_GET, [](AsyncWebServerRequest * request) {handleWebPageStmUpdate(request);});
  server.on("/stmupdstatus", HTTP_GET, [](AsyncWebServerRequest * request) {handleStmUpdStatus(request);});
  server.on("/tempsensorsid", HTTP_GET, [](AsyncWebServerRequest * request) {handleTempSensorsID(request);});
  server.on("/voltsensorsid", HTTP_GET, [](AsyncWebServerRequest * request) {handleVoltSensorsID(request);});
  server.on("/sysconfig", HTTP_GET, [](AsyncWebServerRequest * request) {handleGetSysConfig(request);});
  server.on("/msgconfig", HTTP_GET, [](AsyncWebServerRequest * request) {handleGetMsgConfig(request);});
  server.on("/stm?", HTTP_GET, [](AsyncWebServerRequest * request) {handleGetStm(request);});
  
  server.on("/fupload", HTTP_POST, [](AsyncWebServerRequest *request) {
      // called once after the upload (also when the request carried no file)
      if (uploadSucceeded(request)) request->send(200, aj, Web.getFSDir());
      else request->send(500, tp, "Upload failed");
    },
      [](AsyncWebServerRequest *request, const String& filename, size_t index, uint8_t *data,
                    size_t len, bool final) {handleUploadFile(request, filename, index, data, len, final);}
  );

  AsyncCallbackJsonWebHandler* stmDoUpdateHandler = new AsyncCallbackJsonWebHandler("/stmdoupdate", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      int status = ServerServices.stmDoUpdate (jsonObj);
      if (status == 200) request->send(200, aj, resOk);
      else if (status == 409) request->send(409, tp, "STM update refused: one update per boot or restart pending");
      else request->send(400, tp, "Invalid file or command");
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(stmDoUpdateHandler);

  AsyncCallbackJsonWebHandler* setValveHandler = new AsyncCallbackJsonWebHandler("/setvalve", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      if (ServerServices.postSetValve (jsonObj)) request->send(200, aj, resOk);
      else request->send(400, tp, "Invalid valve or target");
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(setValveHandler);

  AsyncCallbackJsonWebHandler* netCfgHandler = new AsyncCallbackJsonWebHandler("/netconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postNetCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(netCfgHandler);

  AsyncCallbackJsonWebHandler* sysLogCfgHandler = new AsyncCallbackJsonWebHandler("/sysLogCfg", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postSysLogCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(sysLogCfgHandler);  
  
  AsyncCallbackJsonWebHandler* protCfgHandler = new AsyncCallbackJsonWebHandler("/protconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postProtCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(protCfgHandler);

  AsyncCallbackJsonWebHandler* valvesCfgHandler = new AsyncCallbackJsonWebHandler("/valvesconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postValvesCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(valvesCfgHandler);
  
AsyncCallbackJsonWebHandler* valvesControlCfgHandler = new AsyncCallbackJsonWebHandler("/valvesctrlconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postValvesControlCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(valvesControlCfgHandler);
  
  
  AsyncCallbackJsonWebHandler* tempsCfgHandler = new AsyncCallbackJsonWebHandler("/tempsconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postTempsCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(tempsCfgHandler);

  AsyncCallbackJsonWebHandler* voltsCfgHandler = new AsyncCallbackJsonWebHandler("/voltsconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postVoltsCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(voltsCfgHandler);

  AsyncCallbackJsonWebHandler* tempsAuthHandler = new AsyncCallbackJsonWebHandler("/auth", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      String auth=VdmConfig.handleAuth (jsonObj);
      request->send(200, aj, auth);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(tempsAuthHandler);

  AsyncCallbackJsonWebHandler* cmdHandler = new AsyncCallbackJsonWebHandler("/cmd", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      int status = handleCmd (jsonObj);
      if (status == 200) request->send(200, aj, resOk);
      else if (status == 400) request->send(400, tp, "Invalid valve");
      else if (status == 409) request->send(409, tp, "Refused: STM update running");
      else request->send(200, tp, "Cmd not found");       // answered with 200 as always
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(cmdHandler);

  AsyncCallbackJsonWebHandler* sysCfgHandler = new AsyncCallbackJsonWebHandler("/sysconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postSysCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(sysCfgHandler);
  
AsyncCallbackJsonWebHandler* msgCfgHandler = new AsyncCallbackJsonWebHandler("/msgconfig", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      VdmConfig.postMessengerCfg (jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(msgCfgHandler);

AsyncCallbackJsonWebHandler* testPOHandler = new AsyncCallbackJsonWebHandler("/testPO", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      Messenger.testPO(jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(testPOHandler);  

AsyncCallbackJsonWebHandler* testEmailHandler = new AsyncCallbackJsonWebHandler("/testEmail", [](AsyncWebServerRequest *request, JsonVariant &json) {
    if (json.is<JsonObject>()) {
      JsonObject&& jsonObj = json.as<JsonObject>();
      Messenger.testEmail(jsonObj);
      request->send(200, aj, resOk);
    } else request->send(400, tp, "Not an object");
  });
  server.addHandler(testEmailHandler);  

  // protect the ESP firmware upload with the web login, when one is configured
  // (same rule as /auth: both user name and password must be set)
  if ((strlen(VdmConfig.configFlash.netConfig.userName)>0) && (strlen(VdmConfig.configFlash.netConfig.userPwd)>0)) {
    WT32AsyncOTA.begin(&server,VdmConfig.configFlash.netConfig.userName,VdmConfig.configFlash.netConfig.userPwd);
  } else {
    WT32AsyncOTA.begin(&server);
  }
  server.begin();
}


